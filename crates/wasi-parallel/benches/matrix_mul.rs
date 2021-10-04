mod run;
use criterion::{
    criterion_group, criterion_main, measurement::WallTime, BenchmarkGroup, Criterion,
};
use run::BenchContext;
use std::path::Path;

fn bench_nstream(c: &mut Criterion) {
    let mut group = c.benchmark_group("matrix_mul");
    measure_wasm(&mut group, "benches/wasm/matrix_mul.wasm");
}

fn measure_wasm<P: AsRef<Path>>(group: &mut BenchmarkGroup<WallTime>, path: P) {
    let path = path.as_ref();
    group.bench_function(path.to_string_lossy(), |b| {
        b.iter_with_setup(
            || -> BenchContext {
                let mut ctx = BenchContext::new(path).unwrap();
                ctx.invoke("setup", None).unwrap();
                ctx
            },
            |ctx: BenchContext| {
                let mut ctx = ctx;
                ctx.invoke("run", None)
            },
        );
    });
}

criterion_group!(benches, bench_nstream);
criterion_main!(benches);
