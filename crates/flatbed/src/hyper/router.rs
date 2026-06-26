//! Trie-based router for efficient path matching
//!
//! This router supports:
//! - Static paths: `/api/users`
//! - Path parameters: `/api/users/{id}`
//! - Multiple HTTP methods per path

use std::collections::HashMap;

use crate::RouteInfo;

/// A route entry with its handler information
#[derive(Clone)]
pub struct RouteEntry {
    /// The route info from inventory registration
    pub info: RouteInfo,
    /// The hyper handler function
    pub handler: HandlerFn,
}

/// Async handler function type for hyper
///
/// Takes request parts, body bytes, content-type, application context, and
/// returns response parts.
pub type HandlerFn = fn(
    crate::RequestParts,
    Vec<u8>,
    &str,
    std::sync::Arc<dyn std::any::Any + Send + Sync>,
) -> std::pin::Pin<
    Box<dyn std::future::Future<Output = Result<crate::ResponseParts, crate::Error>> + Send>,
>;

/// Router node in the trie
struct RouterNode {
    /// Static children by path segment
    children: HashMap<String, RouterNode>,
    /// Parameter child (matches any segment, stores parameter name)
    param_child: Option<(String, Box<RouterNode>)>,
    /// Handlers by HTTP method
    handlers: HashMap<String, RouteEntry>,
}

impl RouterNode {
    fn new() -> Self {
        Self {
            children: HashMap::new(),
            param_child: None,
            handlers: HashMap::new(),
        }
    }
}

/// Trie-based router for path matching
pub struct Router {
    root: RouterNode,
}

impl Router {
    /// Create a new empty router
    pub fn new() -> Self {
        Self {
            root: RouterNode::new(),
        }
    }

    /// Add a route to the router
    pub fn add_route(&mut self, path: &str, method: &str, entry: RouteEntry) {
        let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let mut node = &mut self.root;

        for segment in segments {
            if segment.starts_with('{') && segment.ends_with('}') {
                // Parameter segment
                let param_name = segment[1..segment.len() - 1].to_string();
                if node.param_child.is_none() {
                    node.param_child = Some((param_name.clone(), Box::new(RouterNode::new())));
                }
                node = node.param_child.as_mut().unwrap().1.as_mut();
            } else {
                // Static segment
                node = node
                    .children
                    .entry(segment.to_string())
                    .or_insert_with(RouterNode::new);
            }
        }

        node.handlers.insert(method.to_uppercase(), entry);
    }

    /// Match a path and return the route entry with extracted parameters
    pub fn match_route(
        &self,
        path: &str,
        method: &str,
    ) -> Option<(&RouteEntry, HashMap<String, String>)> {
        let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let mut params = HashMap::new();
        let mut node = &self.root;

        for segment in &segments {
            // Try static match first
            if let Some(child) = node.children.get(*segment) {
                node = child;
            } else if let Some((param_name, param_node)) = &node.param_child {
                // Try parameter match
                params.insert(param_name.clone(), (*segment).to_string());
                node = param_node.as_ref();
            } else {
                return None;
            }
        }

        // Look up handler by method
        let method_upper = method.to_uppercase();
        node.handlers.get(&method_upper).map(|e| (e, params))
    }

    /// Get all registered methods for a path (for 405 Method Not Allowed responses)
    pub fn get_allowed_methods(&self, path: &str) -> Vec<String> {
        let segments: Vec<&str> = path.split('/').filter(|s| !s.is_empty()).collect();
        let mut node = &self.root;

        for segment in &segments {
            if let Some(child) = node.children.get(*segment) {
                node = child;
            } else if let Some((_, param_node)) = &node.param_child {
                node = param_node.as_ref();
            } else {
                return Vec::new();
            }
        }

        node.handlers.keys().cloned().collect()
    }
}

impl Default for Router {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{HttpMethod, OpenApiRouteInfo, RouteInfo};

    fn make_test_entry() -> RouteEntry {
        RouteEntry {
            info: RouteInfo {
                path: "/test",
                method: HttpMethod::Post,
                request_type: "TestRequest",
                response_type: "TestResponse",
                handler: |_, _| panic!("test handler"),
                async_handler: |_, _, _, _| {
                    Box::pin(async { Err(crate::Error::Custom("test".into())) })
                },
                openapi: OpenApiRouteInfo::default(),
            },
            handler: |_, _, _, _| Box::pin(async { Err(crate::Error::Custom("test".into())) }),
        }
    }

    #[test]
    fn test_static_route_matching() {
        let mut router = Router::new();
        router.add_route("/api/users", "GET", make_test_entry());
        router.add_route("/api/users", "POST", make_test_entry());

        let (_, params) = router.match_route("/api/users", "GET").unwrap();
        assert!(params.is_empty());

        let (_, params) = router.match_route("/api/users", "POST").unwrap();
        assert!(params.is_empty());

        assert!(router.match_route("/api/users", "DELETE").is_none());
        assert!(router.match_route("/api/other", "GET").is_none());
    }

    #[test]
    fn test_parameter_route_matching() {
        let mut router = Router::new();
        router.add_route("/api/users/{id}", "GET", make_test_entry());
        router.add_route("/api/users/{id}/posts/{post_id}", "GET", make_test_entry());

        let (_, params) = router.match_route("/api/users/123", "GET").unwrap();
        assert_eq!(params.get("id"), Some(&"123".to_string()));

        let (_, params) = router
            .match_route("/api/users/456/posts/789", "GET")
            .unwrap();
        assert_eq!(params.get("id"), Some(&"456".to_string()));
        assert_eq!(params.get("post_id"), Some(&"789".to_string()));
    }

    #[test]
    fn test_allowed_methods() {
        let mut router = Router::new();
        router.add_route("/api/users", "GET", make_test_entry());
        router.add_route("/api/users", "POST", make_test_entry());
        router.add_route("/api/users", "PUT", make_test_entry());

        let methods = router.get_allowed_methods("/api/users");
        assert!(methods.contains(&"GET".to_string()));
        assert!(methods.contains(&"POST".to_string()));
        assert!(methods.contains(&"PUT".to_string()));
        assert_eq!(methods.len(), 3);
    }
}
