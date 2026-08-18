//! Tests for [`super::get_or_create_channel`].
//!
//! Covers: valid endpoint, invalid URI, same-endpoint channel reuse, distinct
//! endpoints getting distinct channels, and pool insertion.

use super::*;

fn empty_pool() -> ChannelMap {
    RwLock::new(HashMap::new())
}

#[tokio::test]
async fn valid_endpoint_returns_ok() {
    let pool = empty_pool();
    let result = get_or_create_channel("http://localhost:50051", &pool);
    assert!(result.is_ok());
}

#[tokio::test]
async fn invalid_endpoint_returns_err() {
    let pool = empty_pool();
    // An empty string is not a valid URI.
    let result = get_or_create_channel("", &pool);
    assert!(result.is_err());
}

#[tokio::test]
async fn same_endpoint_reuses_channel() {
    let pool = empty_pool();
    let _ = get_or_create_channel("http://localhost:50051", &pool).unwrap();
    let _ = get_or_create_channel("http://localhost:50051", &pool).unwrap();
    // tonic::transport::Channel doesn't expose pointer equality directly,
    // but we can assert the pool only has one entry — proving no second
    // channel was created.
    assert_eq!(pool.read().unwrap().len(), 1);
}

#[tokio::test]
async fn different_endpoints_get_different_channels() {
    let pool = empty_pool();
    let _ = get_or_create_channel("http://localhost:50051", &pool).unwrap();
    let _ = get_or_create_channel("http://localhost:50052", &pool).unwrap();
    assert_eq!(pool.read().unwrap().len(), 2);
}

#[tokio::test]
async fn channel_is_inserted_into_pool() {
    let pool = empty_pool();
    assert!(pool.read().unwrap().is_empty());
    get_or_create_channel("http://localhost:50051", &pool).unwrap();
    assert!(pool.read().unwrap().contains_key("http://localhost:50051"));
}
