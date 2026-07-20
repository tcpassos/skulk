use contract::{LootKind, LootQuery};
use engine::RedbLoot;
use module_sdk::LootSink;

#[tokio::test]
async fn redb_persists_across_reopen() {
    let path = std::env::temp_dir().join(format!("redbloot-test-{}.redb", std::process::id()));
    let _ = std::fs::remove_file(&path);

    // Write, then drop the store to force everything through disk.
    {
        let store = RedbLoot::open(&path).unwrap();
        store.put(LootKind::Hash, "hash/dc01", b"abc".to_vec()).await.unwrap();
        store.put(LootKind::Pcap, "pcap/eth0", vec![0u8; 100]).await.unwrap();
    }

    // Reopen a brand-new handle to the same file: the loot is still there.
    let store = RedbLoot::open(&path).unwrap();

    let all = store.query(&LootQuery::default()).await.unwrap();
    assert_eq!(all.len(), 2, "both loot items survive a reopen");

    let hashes = store
        .query(&LootQuery { kind: Some(LootKind::Hash), ..Default::default() })
        .await
        .unwrap();
    assert_eq!(hashes.len(), 1);
    assert_eq!(hashes[0].key, "hash/dc01");
    assert_eq!(hashes[0].size, 3, "size excludes the kind byte");

    let by_prefix = store
        .query(&LootQuery { prefix: Some("pcap/".to_string()), ..Default::default() })
        .await
        .unwrap();
    assert_eq!(by_prefix.len(), 1);
    assert_eq!(by_prefix[0].kind, LootKind::Pcap);

    let (kind, bytes) = store.get("hash/dc01").await.unwrap().expect("key exists");
    assert_eq!(kind, LootKind::Hash);
    assert_eq!(bytes, b"abc");

    assert!(store.get("no/such/key").await.unwrap().is_none());

    let _ = std::fs::remove_file(&path);
}
