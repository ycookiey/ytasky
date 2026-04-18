use criterion::{criterion_group, criterion_main, Criterion};
use std::path::Path;
use tempfile::TempDir;
use ybasey::Database;

fn setup_db(dir: &Path) -> Database {
    std::fs::write(dir.join("_meta"), "tz: Asia/Tokyo\nview_sync: async\n").unwrap();
    let mut db = Database::open(dir, Some("bench")).unwrap();
    ytasky::init::apply_schema(&mut db).unwrap();
    db
}

// ---- bench_insert_task --------------------------------------------------

fn bench_insert_task(c: &mut Criterion) {
    c.bench_function("insert_task", |b| {
        b.iter_with_setup(
            || {
                let dir = TempDir::new().unwrap();
                let db = setup_db(dir.path());
                (dir, db)
            },
            |(_dir, mut db)| {
                ytasky::db::insert_task(&mut db, "2026-04-19", "bench task", "1", 30, None)
                    .unwrap()
            },
        )
    });
}

// ---- bench_load_tasks ---------------------------------------------------

fn bench_load_tasks(c: &mut Criterion) {
    let dir = TempDir::new().unwrap();
    let mut db = setup_db(dir.path());
    for i in 0..100 {
        ytasky::db::insert_task(
            &mut db,
            "2026-04-19",
            &format!("task {i}"),
            "1",
            15,
            None,
        )
        .unwrap();
    }

    c.bench_function("load_tasks_100", |b| {
        b.iter(|| ytasky::db::load_tasks(&mut db, "2026-04-19").unwrap())
    });
}

// ---- bench_report_by_category -------------------------------------------

fn bench_report_by_category(c: &mut Criterion) {
    let dir = TempDir::new().unwrap();
    let mut db = setup_db(dir.path());
    for i in 0..50 {
        let cat = ((i % 9) + 1).to_string();
        ytasky::db::insert_task(&mut db, "2026-04-19", &format!("t{i}"), &cat, 30, None).unwrap();
    }

    c.bench_function("report_by_category", |b| {
        b.iter(|| ytasky::db::report_by_category(&db, "2026-04-19").unwrap())
    });
}

// ---- bench_batch_insert_500 --------------------------------------------

fn bench_batch_insert_500(c: &mut Criterion) {
    c.bench_function("batch_insert_500", |b| {
        b.iter_with_setup(
            || {
                let dir = TempDir::new().unwrap();
                let db = setup_db(dir.path());
                (dir, db)
            },
            |(_dir, mut db)| {
                for i in 0..500u32 {
                    ytasky::db::insert_task(
                        &mut db,
                        "2026-04-19",
                        &format!("recur {i}"),
                        "1",
                        30,
                        None,
                    )
                    .unwrap();
                }
            },
        )
    });
}

// ---- Windows/NTFS 特有 bench -------------------------------------------

#[cfg(target_os = "windows")]
fn bench_dir_listing_22000(c: &mut Criterion) {
    use std::io::Write as _;

    let dir = TempDir::new().unwrap();
    // 22000 ファイルを作成
    let n = 22_000usize;
    for i in 0..n {
        let p = dir.path().join(format!("{i:05}.txt"));
        std::fs::File::create(&p)
            .unwrap()
            .write_all(b"x")
            .unwrap();
    }

    c.bench_function("dir_listing_22000", |b| {
        b.iter(|| {
            std::fs::read_dir(dir.path())
                .unwrap()
                .count()
        })
    });
}

#[cfg(target_os = "windows")]
fn bench_dir_mtime_detection(c: &mut Criterion) {
    let dir = TempDir::new().unwrap();

    c.bench_function("dir_mtime_detection", |b| {
        b.iter_with_setup(
            || {
                // ファイル作成してから mtime 取得
                let p = dir.path().join("probe.txt");
                std::fs::write(&p, b"probe").unwrap();
                p
            },
            |p| {
                let meta = std::fs::metadata(dir.path()).unwrap();
                let _ = meta.modified().unwrap();
                std::fs::remove_file(&p).ok();
            },
        )
    });
}

#[cfg(target_os = "windows")]
criterion_group!(
    ytasky_benches,
    bench_insert_task,
    bench_load_tasks,
    bench_report_by_category,
    bench_batch_insert_500,
    bench_dir_listing_22000,
    bench_dir_mtime_detection,
);

#[cfg(not(target_os = "windows"))]
criterion_group!(
    ytasky_benches,
    bench_insert_task,
    bench_load_tasks,
    bench_report_by_category,
    bench_batch_insert_500,
);

criterion_main!(ytasky_benches);
