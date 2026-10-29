//! MCP integration tests (RwLock concurrency layer)
//!
//! Tests are gated by `#[cfg(feature = "mcp")]` so they only run with `--features mcp`.
//! The tests verify RwLock semantics and concurrent write correctness without invoking
//! the actual MCP stdio transport.

#[cfg(feature = "mcp")]
mod mcp_tests {
    use std::sync::Arc;
    use tempfile::TempDir;
    use tokio::sync::RwLock;
    use ybasey::Database;

    fn setup_db(tmp: &TempDir) -> Database {
        std::fs::write(
            tmp.path().join("_meta"),
            "tz: Asia/Tokyo\nview_sync: async\n",
        )
        .unwrap();
        let mut db = Database::open(tmp.path(), Some("mcp-test")).unwrap();
        ytasky::init::apply_schema(&mut db).unwrap();
        db
    }

    #[tokio::test]
    async fn test_mcp_server_build() {
        let tmp = TempDir::new().unwrap();
        let db = setup_db(&tmp);
        let _server = ytasky::mcp::YtaskyMcpServer::new(db);
    }

    /// `Database` is not `Send`, so it cannot go into `tokio::spawn`. The test checks
    /// exclusivity through sequential writes behind `Arc<RwLock<Database>>` instead.
    #[tokio::test]
    async fn test_mcp_concurrent_writes() {
        let tmp = TempDir::new().unwrap();
        let db = setup_db(&tmp);
        let db = Arc::new(RwLock::new(db));

        let mut ids = Vec::new();
        for i in 0..10 {
            let mut locked = db.write().await;
            let id = ytasky::db::insert_task(
                &mut locked,
                "2026-04-19",
                &format!("タスク{i}"),
                "1",
                30,
                None,
            )
            .unwrap();
            ids.push(id);
        }

        let mut unique = ids.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(ids.len(), unique.len(), "id 重複: {:?}", ids);

        let mut locked = db.write().await;
        let tasks = ytasky::db::load_tasks(&mut locked, "2026-04-19").unwrap();
        assert_eq!(tasks.len(), 10);
    }

    #[tokio::test]
    async fn test_mcp_read_during_write() {
        let tmp = TempDir::new().unwrap();
        let db = setup_db(&tmp);
        let db = Arc::new(RwLock::new(db));

        {
            let mut locked = db.write().await;
            ytasky::db::insert_task(&mut locked, "2026-04-20", "READ TEST", "1", 15, None)
                .unwrap();
        }

        {
            let locked = db.read().await;
            let cats = ytasky::db::load_categories(&locked).unwrap();
            assert_eq!(cats.len(), 9);
        }

        // `load_tasks` needs `&mut`, so it goes through a write lock.
        {
            let mut locked = db.write().await;
            let tasks = ytasky::db::load_tasks(&mut locked, "2026-04-20").unwrap();
            assert_eq!(tasks.len(), 1);
            assert_eq!(tasks[0].title, "READ TEST");
        }
    }
}
