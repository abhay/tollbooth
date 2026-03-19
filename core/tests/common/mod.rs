use spl_tollbooth_core::store::LibsqlStore;

/// Create a test database backed by a unique temp file.
/// libsql's `:memory:` databases do NOT share state across `db.connect()`
/// calls, so we use a temp file to ensure all connections see the same data.
pub async fn test_store() -> (LibsqlStore, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("test.db");
    let db = libsql::Builder::new_local(path).build().await.unwrap();
    let store = LibsqlStore::new(db);
    store.run_migrations().await.unwrap();
    (store, dir)
}
