//! The generated `PageRequest` / `PageResponse` impls, driven only through the
//! traits, the way a pager generic over the request and response uses them.

#[path = "../src/generated/test_flatbed.rs"]
#[allow(warnings, clippy::all)]
mod generated;

use flatbed::{PageRequest, PageResponse};
use generated::test::{
    ItemOrder, ItemStatus, ListItemsRequest, ListItemsResponse, ListedItem, ListedItemsFeed, Page,
};

fn first_page<Req: PageRequest, Resp: PageResponse<Req>>(
    request: &Req,
    items: Vec<Resp::Item>,
) -> Resp {
    let mut filters = request.clone();
    let (_, page_size) = filters.take_page();
    filters.set_page_size(page_size.max(1));
    Resp::from_page(items, Some("next".to_string()), None, filters)
}

fn round_trip<Req: PageRequest>(request: &Req) -> Req {
    Req::from_flatbuffer(&request.to_flatbuffer()).expect("flatbuffer decode failed")
}

fn item(id: &str) -> ListedItem {
    ListedItem {
        id: Some(id.to_string()),
        ..Default::default()
    }
}

fn filtered_request() -> ListItemsRequest {
    ListItemsRequest {
        page: Some("cursor".to_string()),
        page_size: 25,
        owner: Some("ana".to_string()),
        status: ItemStatus::Open,
        order: ItemOrder::Oldest,
    }
}

#[test]
fn take_page_clears_both_page_fields_and_keeps_filters() {
    let mut request = filtered_request();

    assert_eq!(request.take_page(), (Some("cursor".to_string()), 25));
    assert_eq!(request.page, None);
    assert_eq!(request.page_size, 0);
    assert_eq!(request.owner.as_deref(), Some("ana"));
    assert_ne!(request, ListItemsRequest::default());
}

#[test]
fn a_request_with_only_page_fields_equals_the_default_once_taken() {
    let mut request = ListItemsRequest {
        page: Some("cursor".to_string()),
        page_size: 10,
        ..Default::default()
    };
    request.take_page();

    assert_eq!(request, ListItemsRequest::default());
}

#[test]
fn filters_round_trip_through_flatbuffers() {
    let mut filters = filtered_request();
    filters.take_page();

    assert_eq!(round_trip(&filters), filters);
}

#[test]
fn a_response_with_an_applied_field_records_the_request() {
    let page: ListItemsResponse = first_page(&filtered_request(), vec![item("a"), item("b")]);

    let mut applied = filtered_request();
    applied.page = None;
    assert_eq!(
        page,
        ListItemsResponse {
            items: Some(vec![item("a"), item("b")]),
            page: Some(Page {
                next_page: Some("next".to_string()),
                previous_page: None,
            }),
            applied: Some(applied),
        }
    );
}

#[test]
fn a_response_without_an_applied_field_pairs_with_any_request() {
    let page: ListedItemsFeed = first_page(&filtered_request(), Vec::new());

    assert_eq!(page.items, Some(Vec::new()));
    assert_eq!(page.total, 0);
    assert_eq!(
        page.page,
        Some(Page {
            next_page: Some("next".to_string()),
            previous_page: None,
        })
    );
}
