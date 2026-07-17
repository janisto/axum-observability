use super::*;

#[tokio::test(flavor = "current_thread")]
async fn missing_request_context_has_a_stable_non_sensitive_rejection() {
    fn assert_standard_traits<T: Clone + Copy + std::fmt::Debug + Eq + std::error::Error>() {}
    assert_standard_traits::<MissingRequestContext>();

    let response = Router::new()
        .route("/", get(context_handler))
        .oneshot(Request::new(Body::empty()))
        .await
        .expect("response");
    assert_eq!(response.status(), StatusCode::INTERNAL_SERVER_ERROR);
    let body = to_bytes(response.into_body(), 1_024).await.expect("body");
    assert_eq!(body, "request context unavailable");
}
