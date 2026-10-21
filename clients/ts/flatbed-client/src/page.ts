/**
 * One page of a paginated list call. `nextPage`/`previousPage` are the raw
 * cursors (for links); `next`/`previous` fetch the adjacent page and are
 * `undefined` at the ends of the list.
 */
export interface Page<T, R = never> {
  readonly items: readonly T[];
  /** The request the page was made from, as the server applied it, when the response echoes one. */
  readonly applied?: R;
  readonly nextPage?: string;
  readonly previousPage?: string;
  readonly next?: () => Promise<Page<T, R>>;
  readonly previous?: () => Promise<Page<T, R>>;
}

/** The parts of a decoded list response a {@link Page} is built from. */
export interface PageParts<T, R> {
  readonly items?: readonly T[];
  readonly page?: { readonly next_page?: string; readonly previous_page?: string };
  readonly applied?: R;
}

const cursor = (c: string | undefined): string | undefined => (c === "" ? undefined : c);

/**
 * Send `body` and wrap the response as a {@link Page}. `next`/`previous` send
 * `atCursor(cursor)`: a cursor already carries the filters it was minted with,
 * and the server rejects a cursor sent alongside any filter.
 */
export const paginate = <Req, T, R>(
  send: (body: Req) => Promise<PageParts<T, R>>,
  body: Req,
  atCursor: (page: string) => Req,
): Promise<Page<T, R>> =>
  send(body).then((parts) => {
    const follow = (page: string | undefined) =>
      page === undefined ? undefined : () => paginate(send, atCursor(page), atCursor);
    const nextPage = cursor(parts.page?.next_page);
    const previousPage = cursor(parts.page?.previous_page);
    return {
      items: parts.items ?? [],
      applied: parts.applied,
      nextPage,
      previousPage,
      next: follow(nextPage),
      previous: follow(previousPage),
    };
  });
