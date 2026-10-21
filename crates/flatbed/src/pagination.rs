//! Cursor pagination for list routes.
//!
//! flatbed owns the page shape, the cursor, and page-size policy; the service
//! owns its filters, its order, and its storage. The service declares a
//! `(paginated)` request/response pair in its schema (see [`PageRequest`] and
//! [`PageResponse`]), builds one [`Pager`] at boot, keeps it in its own
//! context, and calls [`Pager::page`] from an ordinary route:
//!
//! ```rust,ignore
//! #[route("/feedback/list", method = "POST")]
//! async fn list_feedback(
//!     req: Request<ListFeedbackRequest, Arc<AppCtx>>,
//! ) -> Result<Response<ListFeedbackResponse>, FlatbedRouteError> {
//!     let caller = req.header("x-user-id").unwrap_or_default().as_bytes().to_vec();
//!     let page = req.ctx.pager.page(&req, &caller, |q| req.ctx.repo.list(q)).await?;
//!     Ok(Response::ok(page))
//! }
//! ```
//!
//! # The service's side of the contract
//!
//! - **The key `K`** is what the service returns beside each row and gets back
//!   as [`ListQuery::after`]. For every order the request can select, `K` must
//!   be a strict total order matching that `ORDER BY` — end it with a unique
//!   tie-breaker such as the row id — and its fields must never change for a
//!   row. When orders sort on different columns, `K` carries all of them.
//!   flatbed stores `K` as JSON and never looks inside it.
//! - **Direction.** For [`Direction::Forward`] return rows strictly after
//!   `after` in the request's order. For [`Direction::Backward`] flip both the
//!   `ORDER BY` and the comparison, returning rows strictly before `after`
//!   nearest-first; flatbed reverses them back into display order.
//! - **Return at most [`ListQuery::limit`] rows.** The limit is one more than
//!   the page size; the extra row only tells flatbed another page exists.
//! - **Lists are `POST`.** A `GET` route decodes its body from `{}`, so neither
//!   the filters nor the cursor would arrive.
//! - **Optional number filters** are enums whose first variant is `Any = 0`:
//!   a scalar left at its default is indistinguishable from an unset one.
//!
//! # Cursors
//!
//! A cursor carries its direction, the key it continues from, and the
//! request's filters re-encoded as FlatBuffers, so one minted on a JSON call
//! works on a FlatBuffer call and back. Its HMAC-SHA256 tag also covers the
//! HTTP method, the concrete request path, and the caller binding, none of
//! which are stored: a cursor replayed on another route or by another caller
//! is rejected. Cursors are **signed, not encrypted** — anyone holding one can
//! read its filters and key, so neither may carry anything the caller must
//! not see.
//!
//! A cursor rejected for any reason is `400 INVALID_CURSOR`. A request sending
//! both a cursor and any filter other than `page_size` is
//! `400 CURSOR_WITH_FILTERS`: the cursor already carries its filters.
//!
//! # Schema evolution
//!
//! Request filters are append-only: appending a field to the request table
//! keeps old cursors valid — the appended field decodes as its default —
//! while removing or reordering fields changes what old cursors decode to.
//! Changing the shape of `K` invalidates every outstanding cursor
//! (`400 INVALID_CURSOR`); clients restart from the first page.

// Route handlers return `FlatbedRouteError` by value, so `?` on the pager must too.
#![allow(clippy::result_large_err)]

use std::future::Future;

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use hmac::{Hmac, Mac};
use serde::de::DeserializeOwned;
use serde::Serialize;
use sha2::Sha256;

use crate::{FlatbedRouteError, PageRequest, PageResponse, Request};

const VERSION: u8 = 1;
const LABEL: &[u8] = b"flatbed.page-cursor";
const TAG_LEN: usize = 16;
const HEADER_LEN: usize = 4;
const MAX_CURSOR_CHARS: usize = 4096;

/// Error code for a cursor that is malformed, tampered with, oversized, or
/// minted for another route or caller.
pub const INVALID_CURSOR: &str = "INVALID_CURSOR";

/// Error code for a request carrying both a cursor and filters.
pub const CURSOR_WITH_FILTERS: &str = "CURSOR_WITH_FILTERS";

/// Which way a page walks from [`ListQuery::after`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Direction {
    /// Rows after the key, in the request's order.
    Forward,
    /// Rows before the key: the service flips its `ORDER BY` and comparison.
    Backward,
}

/// What the service's fetch closure is asked for.
#[derive(Clone, Debug, PartialEq)]
pub struct ListQuery<Req, K> {
    /// The filters the page is made from — from the body on a first page, from
    /// the cursor afterwards. `page` is `None`; `page_size` is the effective
    /// size.
    pub request: Req,
    /// Which way to read from `after`.
    pub direction: Direction,
    /// The key to continue strictly after (in `direction`), or `None` to
    /// start from the beginning of the order.
    pub after: Option<K>,
    /// The most rows to return: the page size plus one.
    pub limit: u32,
}

/// Builds pages and mints/verifies their cursors. Construct one at boot and
/// keep it in the service context.
#[derive(Clone)]
pub struct Pager {
    mac: Hmac<Sha256>,
    default_page_size: u32,
    max_page_size: u32,
}

impl std::fmt::Debug for Pager {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Pager")
            .field("default_page_size", &self.default_page_size)
            .field("max_page_size", &self.max_page_size)
            .finish_non_exhaustive()
    }
}

impl Pager {
    /// A pager signing cursors with `secret`, with a default page size of 50
    /// capped at 200. Every replica serving the route needs the same secret;
    /// changing it invalidates all outstanding cursors.
    #[must_use]
    pub fn new(secret: impl AsRef<[u8]>) -> Self {
        let mac = <Hmac<Sha256> as Mac>::new_from_slice(secret.as_ref())
            .expect("HMAC accepts keys of any length");
        Self {
            mac,
            default_page_size: 50,
            max_page_size: 200,
        }
    }

    /// The page size used when a request sends `page_size: 0`.
    #[must_use]
    pub fn default_page_size(mut self, size: u32) -> Self {
        self.default_page_size = size;
        self
    }

    /// The largest page served; a larger `page_size` is capped to it.
    #[must_use]
    pub fn max_page_size(mut self, size: u32) -> Self {
        self.max_page_size = size;
        self
    }

    /// Serve one page of `req`.
    ///
    /// `caller` binds cursors to whoever minted them — typically the
    /// authenticated principal's id; pass `&[]` for a public list. `fetch`
    /// returns at most `query.limit` rows, each with its key, honouring the
    /// contract in the [module docs](self).
    ///
    /// Works unchanged from a `#[nats_route]`, whose request carries the
    /// concrete subject as its path.
    ///
    /// # Errors
    ///
    /// `400 INVALID_CURSOR` and `400 CURSOR_WITH_FILTERS` as described in the
    /// module docs, or `fetch`'s own error.
    pub async fn page<Req, Resp, K, C, F, Fut, E>(
        &self,
        req: &Request<Req, C>,
        caller: &[u8],
        fetch: F,
    ) -> Result<Resp, FlatbedRouteError>
    where
        Req: PageRequest,
        Resp: PageResponse<Req>,
        K: Serialize + DeserializeOwned,
        F: FnOnce(ListQuery<Req, K>) -> Fut,
        Fut: Future<Output = Result<Vec<(Resp::Item, K)>, E>>,
        E: Into<FlatbedRouteError>,
    {
        let binding = Binding {
            method: req.method.as_str().as_bytes(),
            path: req.path.as_bytes(),
            caller,
        };
        let mut filters = req.body.clone();
        let (cursor, requested) = filters.take_page();
        let page_size = self.effective_page_size(requested);

        let (filters, direction, after) = match cursor.filter(|c| !c.is_empty()) {
            None => (filters, Direction::Forward, None),
            Some(cursor) => {
                if filters != Req::default() {
                    return Err(FlatbedRouteError::bad_request(
                        "a cursor already carries its filters; drop page to change them",
                    )
                    .code(CURSOR_WITH_FILTERS));
                }
                let (filters, direction, key) = self.open(&cursor, &binding)?;
                (filters, direction, Some(key))
            }
        };
        let from_cursor = after.is_some();

        let mut applied = filters.clone();
        applied.set_page_size(page_size);
        let query = ListQuery {
            request: applied.clone(),
            direction,
            after,
            limit: page_size.saturating_add(1),
        };
        let mut rows = fetch(query).await.map_err(Into::into)?;

        let has_more = rows.len() > page_size as usize;
        rows.truncate(page_size as usize);
        if direction == Direction::Backward {
            rows.reverse();
        }

        let (has_previous, has_next) = match direction {
            Direction::Forward => (from_cursor, has_more),
            Direction::Backward => (has_more, true),
        };
        let mint = |direction, row: Option<&(Resp::Item, K)>, wanted: bool| {
            let (_, key) = row.filter(|_| wanted)?;
            Some(self.mint(direction, key, &filters, &binding))
        };
        let previous = mint(Direction::Backward, rows.first(), has_previous).transpose()?;
        let next = mint(Direction::Forward, rows.last(), has_next).transpose()?;

        let items = rows.into_iter().map(|(item, _)| item).collect();
        Ok(Resp::from_page(items, next, previous, applied))
    }

    fn effective_page_size(&self, requested: u32) -> u32 {
        let size = if requested == 0 {
            self.default_page_size
        } else {
            requested
        };
        size.min(self.max_page_size)
    }

    fn tag(&self, binding: &Binding<'_>, payload: &[u8]) -> Hmac<Sha256> {
        let mut mac = self.mac.clone();
        mac.update(LABEL);
        for part in [binding.method, binding.path, binding.caller, payload] {
            mac.update(&(part.len() as u64).to_be_bytes());
            mac.update(part);
        }
        mac
    }

    fn mint<Req: PageRequest, K: Serialize>(
        &self,
        direction: Direction,
        key: &K,
        filters: &Req,
        binding: &Binding<'_>,
    ) -> Result<String, FlatbedRouteError> {
        let key = serde_json::to_vec(key).map_err(|e| {
            FlatbedRouteError::internal(format!("page key does not serialize: {e}"))
        })?;
        let key_len = u16::try_from(key.len())
            .map_err(|_| FlatbedRouteError::internal("page key exceeds 65535 bytes"))?;
        let mut payload = vec![VERSION, direction.byte()];
        payload.extend_from_slice(&key_len.to_be_bytes());
        payload.extend_from_slice(&key);
        payload.extend_from_slice(&filters.to_flatbuffer());
        let tag = self.tag(binding, &payload).finalize().into_bytes();
        payload.extend_from_slice(&tag[..TAG_LEN]);
        Ok(URL_SAFE_NO_PAD.encode(payload))
    }

    fn open<Req: PageRequest, K: DeserializeOwned>(
        &self,
        cursor: &str,
        binding: &Binding<'_>,
    ) -> Result<(Req, Direction, K), FlatbedRouteError> {
        let invalid = || FlatbedRouteError::bad_request("invalid page cursor").code(INVALID_CURSOR);
        if cursor.len() > MAX_CURSOR_CHARS {
            return Err(invalid());
        }
        let bytes = URL_SAFE_NO_PAD.decode(cursor).map_err(|_| invalid())?;
        let Some(split) = bytes
            .len()
            .checked_sub(TAG_LEN)
            .filter(|&n| n >= HEADER_LEN)
        else {
            return Err(invalid());
        };
        let (payload, tag) = bytes.split_at(split);
        self.tag(binding, payload)
            .verify_truncated_left(tag)
            .map_err(|_| invalid())?;

        let [version, direction, len_hi, len_lo, rest @ ..] = payload else {
            return Err(invalid());
        };
        let direction = match (*version, *direction) {
            (VERSION, 0) => Direction::Forward,
            (VERSION, 1) => Direction::Backward,
            _ => return Err(invalid()),
        };
        let key_len = usize::from(u16::from_be_bytes([*len_hi, *len_lo]));
        if rest.len() < key_len {
            return Err(invalid());
        }
        let (key, filters) = rest.split_at(key_len);
        let key = serde_json::from_slice(key).map_err(|_| invalid())?;
        let filters = Req::from_flatbuffer(filters).map_err(|_| invalid())?;
        Ok((filters, direction, key))
    }
}

impl Direction {
    fn byte(self) -> u8 {
        match self {
            Self::Forward => 0,
            Self::Backward => 1,
        }
    }
}

struct Binding<'a> {
    method: &'a [u8],
    path: &'a [u8],
    caller: &'a [u8],
}
