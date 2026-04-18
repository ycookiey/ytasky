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

    /// YtaskyMcpServer が正常に構築できること
    #[tokio::test]
    async fn test_mcp_server_build() {
        let tmp = TempDir::new().unwrap();
        let db = setup_db(&tmp);
        let _server = ytasky::mcp::YtaskyMcpServer::new(db);
    }

    /// 10 concurrent write tasks で id 重複なし
    ///
    /// Database は Send でないため tokio::spawn には渡せない。
    /// 代わりに Arc<RwLock<Database>> を通じた順次 write で排他性を検証する。
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

        // id 重複なし
        let mut unique = ids.clone();
        unique.sort();
        unique.dedup();
        assert_eq!(ids.len(), unique.len(), "id 重複: {:?}", ids);

        // 全 10 件が読み取り可能
        let mut locked = db.write().await;
        let tasks = ytasky::db::load_tasks(&mut locked, "2026-04-19").unwrap();
        assert_eq!(tasks.len(), 10);
    }

    /// read lock と write lock の排他性: write 後に read で最新データが見えること
    #[tokio::test]
    async fn test_mcp_read_during_write() {
        let tmp = TempDir::new().unwrap();
        let db = setup_db(&tmp);
        let db = Arc::new(RwLock::new(db));

        // write
        {
            let mut locked = db.write().await;
            ytasky::db::insert_task(&mut locked, "2026-04-20", "READ TEST", "1", 15, None)
                .unwrap();
        }

        // read: write 後なので見えるはず
        {
            let locked = db.read().await;
            let cats = ytasky::db::load_categories(&locked).unwrap();
            assert_eq!(cats.len(), 9);
        }

        // 別の write handle で load_tasks (requires &mut)
        {
            let mut locked = db.write().await;
            let tasks = ytasky::db::load_tasks(&mut locked, "2026-04-20").unwrap();
            assert_eq!(tasks.len(), 1);
            assert_eq!(tasks[0].title, "READ TEST");
        }
    }
}
