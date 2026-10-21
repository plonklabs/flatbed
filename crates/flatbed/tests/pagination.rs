//! `Pager` end to end: list routes served over HTTP in both codecs, backed by
//! an in-memory store whose rows tie on `created_at`.

#[path = "../src/generated/test_flatbed.rs"]
#[allow(warnings, clippy::all)]
mod generated;

use std::collections::HashMap;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::Engine;
use flatbed::{
    route, Direction, Flatbed, FlatbedConfig, FlatbedRouteError, HeaderMap, ListQuery, Method,
    Pager, Request, Response,
};
use generated::test::{
    ItemOrder, ItemStatus, ListItemsRequest, ListItemsRequestV2, ListItemsResponse, ListedItem,
    ListedItemsFeed,
};

type Key = (i64, String);

const SECRET: &[u8] = b"pagination-test-secret";
const DEFAULT_PAGE_SIZE: u32 = 3;
const MAX_PAGE_SIZE: u32 = 7;

#[derive(Clone)]
struct Ctx {
    pager: Pager,
    store: Arc<Store>,
}

#[derive(Default)]
struct Store {
    rows: Mutex<Vec<ListedItem>>,
    last_limit: AtomicU32,
}

impl Store {
    fn insert(&self, item: ListedItem) {
        self.rows.lock().unwrap().push(item);
    }

    /// A storage layer honouring the `ListQuery` contract: filter, order by the
    /// key in the requested order (flipped for `Backward`), seek strictly past
    /// `after`, and stop at `limit`.
    fn select(&self, q: ListQuery<ListItemsRequest, Key>) -> Vec<(ListedItem, Key)> {
        self.last_limit.store(q.limit, Ordering::SeqCst);
        let descending =
            (q.request.order == ItemOrder::Newest) == (q.direction == Direction::Forward);
        let mut rows: Vec<(ListedItem, Key)> = self
            .rows
            .lock()
            .unwrap()
            .iter()
            .filter(|row| q.request.owner.is_none() || row.owner == q.request.owner)
            .filter(|row| q.request.status == ItemStatus::Any || row.status == q.request.status)
            .map(|row| (row.clone(), key(row)))
            .collect();
        rows.sort_by(|a, b| a.1.cmp(&b.1));
        if descending {
            rows.reverse();
        }
        rows.into_iter()
            .filter(|(_, k)| match &q.after {
                None => true,
                Some(after) if descending => k < after,
                Some(after) => k > after,
            })
            .take(q.limit as usize)
            .collect()
    }
}

fn key(row: &ListedItem) -> Key {
    (row.created_at, row.id.clone().unwrap())
}

#[allow(clippy::result_large_err)]
async fn serve(
    req: Request<ListItemsRequest, Arc<Ctx>>,
) -> Result<Response<ListItemsResponse>, FlatbedRouteError> {
    let caller = req
        .header("x-caller")
        .unwrap_or_default()
        .as_bytes()
        .to_vec();
    let ctx = &req.ctx;
    let page = ctx
        .pager
        .page(&req, &caller, |q| async move {
            Ok::<_, FlatbedRouteError>(ctx.store.select(q))
        })
        .await?;
    Ok(Response::ok(page))
}

#[route("/items")]
async fn list_items(
    req: Request<ListItemsRequest, Arc<Ctx>>,
) -> Result<Response<ListItemsResponse>, FlatbedRouteError> {
    serve(req).await
}

#[route("/archived-items")]
async fn list_archived_items(
    req: Request<ListItemsRequest, Arc<Ctx>>,
) -> Result<Response<ListItemsResponse>, FlatbedRouteError> {
    serve(req).await
}

fn pager() -> Pager {
    Pager::new(SECRET)
        .default_page_size(DEFAULT_PAGE_SIZE)
        .max_page_size(MAX_PAGE_SIZE)
}

fn item(id: &str, owner: &str, created_at: i64, status: ItemStatus) -> ListedItem {
    ListedItem {
        id: Some(id.to_string()),
        owner: Some(owner.to_string()),
        created_at,
        status,
    }
}

/// Seventeen rows for `ana` (pairs share `created_at`, so only the id breaks
/// ties), alternating Open/Closed, plus rows for `bo` that filters exclude.
fn seed(store: &Store) {
    for n in 0..17 {
        let status = if n % 2 == 0 {
            ItemStatus::Open
        } else {
            ItemStatus::Closed
        };
        store.insert(item(&format!("a{n:02}"), "ana", 100 + n / 2, status));
    }
    for n in 0..4 {
        store.insert(item(&format!("b{n:02}"), "bo", 100 + n, ItemStatus::Open));
    }
}

#[derive(Clone, Copy, Debug)]
enum Codec {
    Json,
    FlatBuffer,
}

const CODECS: [Codec; 2] = [Codec::Json, Codec::FlatBuffer];

struct Server {
    base: String,
    store: Arc<Store>,
    client: reqwest::Client,
    handle: tokio::task::JoinHandle<Result<(), flatbed::Error>>,
}

impl Drop for Server {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

async fn boot() -> Server {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let port = listener.local_addr().unwrap().port();
    drop(listener);

    let store = Arc::new(Store::default());
    seed(&store);
    let ctx = Ctx {
        pager: pager(),
        store: store.clone(),
    };
    let config = FlatbedConfig::new("Pagination")
        .host("127.0.0.1")
        .port(port);
    let handle = tokio::spawn(async move { Flatbed::run(config, |_| async { Ok(ctx) }).await });

    let base = format!("http://127.0.0.1:{port}");
    let client = reqwest::Client::new();
    for _ in 0..100 {
        let probe = client
            .post(format!("{base}/items"))
            .header("content-type", "application/json")
            .body(r#"{"page_size":1}"#);
        if let Ok(r) = probe.send().await {
            if r.status().is_success() {
                return Server {
                    base,
                    store,
                    client,
                    handle,
                };
            }
        }
        tokio::time::sleep(std::time::Duration::from_millis(20)).await;
    }
    panic!("server did not become ready");
}

#[derive(Debug, PartialEq)]
struct Rejection {
    status: u16,
    code: String,
}

impl Server {
    async fn list(
        &self,
        codec: Codec,
        body: &ListItemsRequest,
    ) -> Result<ListItemsResponse, Rejection> {
        self.list_at(codec, "/items", "ana", body).await
    }

    async fn list_at(
        &self,
        codec: Codec,
        path: &str,
        caller: &str,
        body: &ListItemsRequest,
    ) -> Result<ListItemsResponse, Rejection> {
        let request = self
            .client
            .post(format!("{}{path}", self.base))
            .header("x-caller", caller);
        let request = match codec {
            Codec::Json => request
                .header("content-type", "application/json")
                .body(serde_json::to_vec(body).unwrap()),
            Codec::FlatBuffer => request
                .header("content-type", "application/x-flatbuffers")
                .body(body.to_flatbuffer()),
        };
        let response = request.send().await.unwrap();
        let status = response.status().as_u16();
        if status != 200 {
            let code = match codec {
                Codec::Json => {
                    let body: serde_json::Value = response.json().await.unwrap();
                    body["code"].as_str().unwrap_or_default().to_string()
                }
                Codec::FlatBuffer => response
                    .headers()
                    .get("x-error-code")
                    .map(|v| v.to_str().unwrap().to_string())
                    .unwrap_or_default(),
            };
            return Err(Rejection { status, code });
        }
        let bytes = response.bytes().await.unwrap();
        Ok(match codec {
            Codec::Json => serde_json::from_slice(&bytes).unwrap(),
            Codec::FlatBuffer => ListItemsResponse::from_flatbuffer(&bytes).unwrap(),
        })
    }
}

fn ids(page: &ListItemsResponse) -> Vec<String> {
    page.items
        .as_ref()
        .unwrap()
        .iter()
        .map(|i| i.id.clone().unwrap())
        .collect()
}

fn next(page: &ListItemsResponse) -> Option<String> {
    page.page.as_ref().unwrap().next_page.clone()
}

fn previous(page: &ListItemsResponse) -> Option<String> {
    page.page.as_ref().unwrap().previous_page.clone()
}

fn cursor_only(cursor: String, page_size: u32) -> ListItemsRequest {
    ListItemsRequest {
        page: Some(cursor),
        page_size,
        ..Default::default()
    }
}

fn ana(order: ItemOrder, page_size: u32) -> ListItemsRequest {
    ListItemsRequest {
        owner: Some("ana".to_string()),
        order,
        page_size,
        ..Default::default()
    }
}

fn expected_order(store: &Store, order: ItemOrder) -> Vec<String> {
    let mut rows: Vec<Key> = store
        .rows
        .lock()
        .unwrap()
        .iter()
        .filter(|r| r.owner.as_deref() == Some("ana"))
        .map(key)
        .collect();
    rows.sort();
    if order == ItemOrder::Newest {
        rows.reverse();
    }
    rows.into_iter().map(|(_, id)| id).collect()
}

fn invalid_cursor() -> Rejection {
    Rejection {
        status: 400,
        code: "INVALID_CURSOR".to_string(),
    }
}

async fn first_cursor(server: &Server, codec: Codec) -> String {
    let page = server
        .list(codec, &ana(ItemOrder::Newest, 4))
        .await
        .unwrap();
    next(&page).unwrap()
}

#[tokio::test]
async fn walks_forward_to_the_end_and_back_to_the_start() {
    let server = boot().await;
    for codec in CODECS {
        for order in [ItemOrder::Newest, ItemOrder::Oldest] {
            let expected = expected_order(&server.store, order);

            let mut page = server.list(codec, &ana(order, 4)).await.unwrap();
            assert_eq!(previous(&page), None, "first page has no previous");
            let mut forward = vec![ids(&page)];
            while let Some(cursor) = next(&page) {
                page = server.list(codec, &cursor_only(cursor, 4)).await.unwrap();
                assert!(previous(&page).is_some(), "a later page has a previous");
                forward.push(ids(&page));
                assert!(forward.len() <= 5, "the walk overran the list");
            }
            assert_eq!(forward.concat(), expected, "{codec:?} {order:?} forward");
            assert_eq!(forward.len(), 5);

            let mut backward = vec![ids(&page)];
            while let Some(cursor) = previous(&page) {
                page = server.list(codec, &cursor_only(cursor, 4)).await.unwrap();
                assert!(next(&page).is_some(), "an earlier page has a next");
                backward.push(ids(&page));
                assert!(backward.len() <= 5, "the walk overran the list");
            }
            backward.reverse();
            assert_eq!(backward.concat(), expected, "{codec:?} {order:?} backward");
            assert_eq!(
                backward[0].len(),
                4,
                "walking back ends on a full first page"
            );
        }
    }
}

#[tokio::test]
async fn rows_inserted_mid_walk_neither_skip_nor_repeat_existing_rows() {
    let server = boot().await;
    let before = expected_order(&server.store, ItemOrder::Oldest);

    let mut page = server
        .list(Codec::Json, &ana(ItemOrder::Oldest, 4))
        .await
        .unwrap();
    let mut seen = ids(&page);
    server
        .store
        .insert(item("a-early", "ana", 1, ItemStatus::Open));
    server
        .store
        .insert(item("a-late", "ana", 1_000, ItemStatus::Open));
    server
        .store
        .insert(item("a01b", "ana", 100, ItemStatus::Closed));
    while let Some(cursor) = next(&page) {
        page = server
            .list(Codec::Json, &cursor_only(cursor, 4))
            .await
            .unwrap();
        seen.extend(ids(&page));
        assert!(seen.len() <= 20, "the walk overran the list");
    }

    let mut expected = before.clone();
    expected.push("a-late".to_string());
    assert_eq!(
        seen, expected,
        "rows sorting before the cursor stay unseen; rows after it appear in place"
    );
}

#[tokio::test]
async fn an_empty_list_has_no_cursors() {
    let server = boot().await;
    for codec in CODECS {
        let body = ListItemsRequest {
            owner: Some("nobody".to_string()),
            ..Default::default()
        };
        let page = server.list(codec, &body).await.unwrap();
        assert_eq!(ids(&page), Vec::<String>::new());
        assert_eq!((next(&page), previous(&page)), (None, None));
    }
}

#[tokio::test]
async fn a_tampered_cursor_is_rejected() {
    let server = boot().await;
    for codec in CODECS {
        let cursor = first_cursor(&server, codec).await;
        let bytes = URL_SAFE_NO_PAD.decode(&cursor).unwrap();
        for at in [1, 5, bytes.len() / 2, bytes.len() - 1] {
            let mut flipped = bytes.clone();
            flipped[at] ^= 0x01;
            let body = cursor_only(URL_SAFE_NO_PAD.encode(flipped), 4);
            assert_eq!(
                server.list(codec, &body).await.unwrap_err(),
                invalid_cursor(),
                "{codec:?} byte {at}"
            );
        }
    }
}

#[tokio::test]
async fn a_cursor_is_bound_to_its_route_and_caller() {
    let server = boot().await;
    for codec in CODECS {
        let body = cursor_only(first_cursor(&server, codec).await, 4);

        let other_route = server.list_at(codec, "/archived-items", "ana", &body).await;
        assert_eq!(other_route.unwrap_err(), invalid_cursor());

        let other_caller = server.list_at(codec, "/items", "bo", &body).await;
        assert_eq!(other_caller.unwrap_err(), invalid_cursor());

        assert!(server.list_at(codec, "/items", "ana", &body).await.is_ok());
    }
}

#[tokio::test]
async fn malformed_and_oversized_cursors_are_rejected() {
    let server = boot().await;
    let valid = first_cursor(&server, Codec::Json).await;
    let cases = [
        "not base64 at all!".to_string(),
        "AAAA".to_string(),
        URL_SAFE_NO_PAD.encode([0u8; 20]),
        format!("{valid}=="),
        "A".repeat(4097),
        format!("{valid}{}", "A".repeat(4096 - valid.len() + 1)),
    ];
    for codec in CODECS {
        for cursor in &cases {
            let result = server.list(codec, &cursor_only(cursor.clone(), 4)).await;
            assert_eq!(
                result.unwrap_err(),
                invalid_cursor(),
                "{codec:?} {cursor:.24}"
            );
        }
    }
}

#[tokio::test]
async fn page_size_defaults_and_caps() {
    let server = boot().await;
    for codec in CODECS {
        let page = server
            .list(codec, &ana(ItemOrder::Newest, 0))
            .await
            .unwrap();
        assert_eq!(ids(&page).len(), DEFAULT_PAGE_SIZE as usize);
        assert_eq!(page.applied.unwrap().page_size, DEFAULT_PAGE_SIZE);
        assert_eq!(
            server.store.last_limit.load(Ordering::SeqCst),
            DEFAULT_PAGE_SIZE + 1
        );

        let page = server
            .list(codec, &ana(ItemOrder::Newest, 10_000))
            .await
            .unwrap();
        assert_eq!(ids(&page).len(), MAX_PAGE_SIZE as usize);
        assert_eq!(page.applied.unwrap().page_size, MAX_PAGE_SIZE);
        assert_eq!(
            server.store.last_limit.load(Ordering::SeqCst),
            MAX_PAGE_SIZE + 1
        );
    }
}

#[tokio::test]
async fn a_cursor_with_filters_is_rejected_but_page_size_may_change() {
    let server = boot().await;
    for codec in CODECS {
        let cursor = first_cursor(&server, codec).await;

        let filtered = ListItemsRequest {
            status: ItemStatus::Closed,
            ..cursor_only(cursor.clone(), 4)
        };
        assert_eq!(
            server.list(codec, &filtered).await.unwrap_err(),
            Rejection {
                status: 400,
                code: "CURSOR_WITH_FILTERS".to_string(),
            }
        );

        let resized = server.list(codec, &cursor_only(cursor, 2)).await.unwrap();
        assert_eq!(ids(&resized).len(), 2);
        assert_eq!(resized.applied.unwrap().page_size, 2);
    }
}

#[tokio::test]
async fn an_empty_page_string_is_no_cursor() {
    let server = boot().await;
    let body = ListItemsRequest {
        page: Some(String::new()),
        ..ana(ItemOrder::Oldest, 4)
    };
    let page = server.list(Codec::Json, &body).await.unwrap();
    assert_eq!(previous(&page), None);
    assert_eq!(ids(&page)[0], "a00");
}

#[tokio::test]
async fn applied_echoes_body_filters_then_cursor_filters() {
    let server = boot().await;
    for codec in CODECS {
        let body = ListItemsRequest {
            owner: Some("ana".to_string()),
            status: ItemStatus::Open,
            order: ItemOrder::Oldest,
            page_size: 2,
            page: None,
        };
        let first = server.list(codec, &body).await.unwrap();
        assert_eq!(first.applied.as_ref(), Some(&body));
        assert_eq!(ids(&first), ["a00", "a02"]);

        let second = server
            .list(codec, &cursor_only(next(&first).unwrap(), 3))
            .await
            .unwrap();
        let expected = ListItemsRequest {
            page_size: 3,
            ..body
        };
        assert_eq!(second.applied, Some(expected));
        assert_eq!(ids(&second), ["a04", "a06", "a08"]);
    }
}

#[tokio::test]
async fn a_cursor_works_across_codecs() {
    let server = boot().await;
    for (minted, used) in [
        (Codec::Json, Codec::FlatBuffer),
        (Codec::FlatBuffer, Codec::Json),
    ] {
        let first = server
            .list(minted, &ana(ItemOrder::Oldest, 4))
            .await
            .unwrap();
        let second = server
            .list(used, &cursor_only(next(&first).unwrap(), 4))
            .await
            .unwrap();
        assert_eq!(ids(&second), ["a04", "a05", "a06", "a07"]);
    }
}

#[tokio::test]
async fn a_cursor_holds_only_its_header_key_filters_and_tag() {
    let server = boot().await;
    let body = ListItemsRequest {
        owner: Some("ana".to_string()),
        status: ItemStatus::Closed,
        order: ItemOrder::Oldest,
        page_size: 2,
        page: None,
    };
    let first = server.list(Codec::Json, &body).await.unwrap();
    let bytes = URL_SAFE_NO_PAD.decode(next(&first).unwrap()).unwrap();

    let key_len = usize::from(u16::from_be_bytes([bytes[2], bytes[3]]));
    let (header, rest) = bytes.split_at(4);
    let (key_bytes, rest) = rest.split_at(key_len);
    let (filters, tag) = rest.split_at(rest.len() - 16);

    assert_eq!(header[..2], [1, 0], "version 1, forward");
    let last = first.items.as_ref().unwrap().last().unwrap();
    assert_eq!(key_bytes, serde_json::to_vec(&key(last)).unwrap());
    let stored = ListItemsRequest {
        page_size: 0,
        ..body
    };
    assert_eq!(filters, stored.to_flatbuffer());
    assert_eq!(tag.len(), 16);
}

fn direct<Req>(body: Req) -> Request<Req> {
    Request {
        body,
        ctx: (),
        headers: HeaderMap::new(),
        method: Method::POST,
        path: "/items".to_string(),
        path_params: HashMap::new(),
        query_params: HashMap::new(),
        request_id: String::new(),
    }
}

fn v1_query(q: ListQuery<ListItemsRequestV2, Key>) -> ListQuery<ListItemsRequest, Key> {
    ListQuery {
        request: ListItemsRequest {
            page: q.request.page,
            page_size: q.request.page_size,
            owner: q.request.owner,
            status: q.request.status,
            order: q.request.order,
        },
        direction: q.direction,
        after: q.after,
        limit: q.limit,
    }
}

#[tokio::test]
async fn a_cursor_minted_on_an_older_request_table_pages_on_a_newer_one() {
    let store = Store::default();
    seed(&store);
    let pager = pager();

    let v1 = direct(ListItemsRequest {
        owner: Some("ana".to_string()),
        status: ItemStatus::Open,
        order: ItemOrder::Oldest,
        page_size: 2,
        page: None,
    });
    let first: ListedItemsFeed = pager
        .page(&v1, b"ana", |q| async {
            Ok::<_, FlatbedRouteError>(store.select(q))
        })
        .await
        .unwrap();
    let cursor = first.page.unwrap().next_page.unwrap();

    let v2 = direct(ListItemsRequestV2 {
        page: Some(cursor),
        page_size: 2,
        ..Default::default()
    });
    let mut seen = None;
    let second: ListedItemsFeed = pager
        .page(&v2, b"ana", |q: ListQuery<ListItemsRequestV2, Key>| {
            seen = Some(q.request.clone());
            async { Ok::<_, FlatbedRouteError>(store.select(v1_query(q))) }
        })
        .await
        .unwrap();

    let seen = seen.unwrap();
    assert_eq!(seen.owner.as_deref(), Some("ana"));
    assert_eq!(seen.status, ItemStatus::Open);
    assert_eq!(seen.order, ItemOrder::Oldest);
    assert_eq!(seen.label, None, "the appended field reads as its default");
    let ids: Vec<_> = second
        .items
        .unwrap()
        .into_iter()
        .map(|i| i.id.unwrap())
        .collect();
    assert_eq!(ids, ["a04", "a06"]);
}

#[tokio::test]
async fn a_cursor_whose_key_shape_changed_is_rejected() {
    let store = Store::default();
    seed(&store);
    let pager = pager();

    let first: ListedItemsFeed = pager
        .page(&direct(ana(ItemOrder::Oldest, 2)), b"", |q| async {
            Ok::<_, FlatbedRouteError>(store.select(q))
        })
        .await
        .unwrap();
    let cursor = first.page.unwrap().next_page.unwrap();

    let result: Result<ListedItemsFeed, _> = pager
        .page(
            &direct(cursor_only(cursor, 2)),
            b"",
            |_q: ListQuery<_, String>| async { Ok::<_, FlatbedRouteError>(Vec::new()) },
        )
        .await;
    let error = result.unwrap_err();
    assert_eq!(
        (error.status.as_u16(), error.code.as_str()),
        (400, "INVALID_CURSOR")
    );
}

#[tokio::test]
async fn a_signed_cursor_over_4096_chars_is_rejected() {
    let pager = pager();
    let rows = |len: usize| {
        vec![
            (ListedItem::default(), "k".repeat(len)),
            (ListedItem::default(), "k".repeat(len)),
        ]
    };
    for (key_len, accepted) in [(2_000, true), (3_200, false)] {
        let first: ListedItemsFeed = pager
            .page(
                &direct(ana(ItemOrder::Oldest, 1)),
                b"",
                |_q: ListQuery<_, String>| async { Ok::<_, FlatbedRouteError>(rows(key_len)) },
            )
            .await
            .unwrap();
        let cursor = first.page.unwrap().next_page.unwrap();
        assert_eq!(cursor.len() > 4096, !accepted);

        let result: Result<ListedItemsFeed, _> = pager
            .page(
                &direct(cursor_only(cursor, 1)),
                b"",
                |_q: ListQuery<_, String>| async { Ok::<_, FlatbedRouteError>(Vec::new()) },
            )
            .await;
        match result {
            Ok(_) => assert!(accepted, "key of {key_len} chars"),
            Err(e) => assert_eq!((accepted, e.code.as_str()), (false, "INVALID_CURSOR")),
        }
    }
}
