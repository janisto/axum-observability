use super::*;

#[tokio::test(flavor = "current_thread")]
async fn observed_body_delegates_stream_state_and_size_hint() {
    let config = ObservabilityConfig::default();
    let capture = Capture::default();
    let _guard = subscriber(&config, capture).set_default();
    let app = Router::new()
        .route("/full", get(|| async { "hello" }))
        .route("/empty", get(|| async { Body::empty() }))
        .layer(ObservabilityLayer::new(config));

    let full = app
        .clone()
        .oneshot(
            Request::builder()
                .uri("/full")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert!(!full.body().is_end_stream());
    assert_eq!(full.body().size_hint().exact(), Some(5));

    let empty = app
        .oneshot(
            Request::builder()
                .uri("/empty")
                .body(Body::empty())
                .expect("request"),
        )
        .await
        .expect("response");
    assert!(empty.body().is_end_stream());
    assert_eq!(empty.body().size_hint().exact(), Some(0));
}

#[tokio::test(flavor = "current_thread")]
async fn poll_ready_errors_are_delegated_to_the_inner_service() {
    #[derive(Clone)]
    struct ReadinessFailure;

    impl Service<Request<Body>> for ReadinessFailure {
        type Response = Response<Body>;
        type Error = io::Error;
        type Future = std::future::Ready<Result<Self::Response, Self::Error>>;

        fn poll_ready(&mut self, _context: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
            Poll::Ready(Err(io::Error::other("not ready")))
        }

        fn call(&mut self, _request: Request<Body>) -> Self::Future {
            unreachable!("call must not run when readiness fails")
        }
    }

    let mut service = ObservabilityLayer::default().layer(ReadinessFailure);
    let error = std::future::poll_fn(|context| service.poll_ready(context))
        .await
        .expect_err("readiness failure");
    assert_eq!(error.to_string(), "not ready");
}
