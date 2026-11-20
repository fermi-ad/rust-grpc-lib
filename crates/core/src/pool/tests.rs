//! Tests for [`super::get_or_create_channel`].
//!
//! Covers: invalid URI, the runtime check, and endpoint key normalization
//! (which endpoints share an entry and which stay distinct).

use super::*;

fn empty_pool() -> ChannelMap {
    RwLock::new(HashMap::new())
}

// Number of pool entries after requesting each endpoint in order.
fn entries_after(endpoints: &[&str]) -> usize {
    let pool = empty_pool();
    for endpoint in endpoints {
        get_or_create_channel(endpoint, &pool).unwrap();
    }
    pool.read().unwrap().len()
}

#[tokio::test]
async fn trailing_slash_on_pathless_url_shares_entry() {
    assert_eq!(entries_after(&["http://h:50051", "http://h:50051/"]), 1);
    assert_eq!(entries_after(&["http://h:50051/", "http://h:50051"]), 1);
}

#[tokio::test]
async fn scheme_and_host_case_share_entry() {
    assert_eq!(
        entries_after(&[
            "http://host:50051",
            "HTTP://host:50051",
            "http://HOST:50051",
            "HTTP://Host:50051/",
        ]),
        1
    );
}

#[tokio::test]
async fn key_is_lowercased_scheme_and_host_without_lone_slash() {
    let pool = empty_pool();
    get_or_create_channel("HTTP://Host.Example:50051/", &pool).unwrap();
    assert!(
        pool.read()
            .unwrap()
            .contains_key("http://host.example:50051")
    );
}

#[tokio::test]
async fn distinct_ports_stay_distinct() {
    assert_eq!(entries_after(&["http://h:50051", "http://h:50052"]), 2);
}

#[tokio::test]
async fn default_port_is_not_stripped() {
    assert_eq!(entries_after(&["http://h", "http://h:80"]), 2);
}

#[tokio::test]
async fn distinct_schemes_stay_distinct() {
    assert_eq!(entries_after(&["http://h:50051", "https://h:50051"]), 2);
}

#[tokio::test]
async fn distinct_paths_stay_distinct() {
    assert_eq!(
        entries_after(&[
            "http://h:50051",
            "http://h:50051/api",
            "http://h:50051/api/"
        ]),
        3
    );
}

#[tokio::test]
async fn query_stays_distinct() {
    assert_eq!(
        entries_after(&[
            "http://h:50051",
            "http://h:50051/?a=1",
            "http://h:50051/?a=2"
        ]),
        3
    );
}

#[tokio::test]
async fn userinfo_stays_distinct_and_case_sensitive() {
    assert_eq!(
        entries_after(&[
            "http://h:50051",
            "http://user@h:50051",
            "http://User@h:50051",
            "http://user@H:50051",
        ]),
        3
    );
}

#[tokio::test]
async fn invalid_uri_still_errors_after_normalization() {
    let pool = empty_pool();
    assert!(matches!(
        get_or_create_channel("http://exa mple:50051", &pool),
        Err(ChannelError::Transport(_))
    ));
    assert!(matches!(
        get_or_create_channel("", &pool),
        Err(ChannelError::Transport(_))
    ));
    assert!(pool.read().unwrap().is_empty());
}

#[test]
fn outside_runtime_still_errors_even_when_pooled_equivalent_exists() {
    let pool = empty_pool();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    runtime.block_on(async {
        get_or_create_channel("http://h:50051", &pool).unwrap();
    });
    assert!(matches!(
        get_or_create_channel("http://h:50051/", &pool),
        Err(ChannelError::Runtime(_))
    ));
}
