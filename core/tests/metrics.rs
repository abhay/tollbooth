mod common;

#[tokio::test]
async fn increment_metric() {
    let (store, _dir) = common::test_store().await;

    store
        .increment_metric("payments.mpp.charge", 1.0)
        .await
        .unwrap();
    store
        .increment_metric("payments.mpp.charge", 1.0)
        .await
        .unwrap();

    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap()
        .as_secs() as i64;
    let points = store
        .query_metrics("payments.mpp.charge", now - 120, now + 120)
        .await
        .unwrap();
    assert_eq!(points.len(), 1);
    assert_eq!(points[0].value, 2.0);
}

#[tokio::test]
async fn rollup_metrics_no_crash() {
    let (store, _dir) = common::test_store().await;

    store.increment_metric("test.metric", 5.0).await.unwrap();
    // Rollup should succeed even if there's nothing old enough to roll up
    store.rollup_metrics().await.unwrap();
}
