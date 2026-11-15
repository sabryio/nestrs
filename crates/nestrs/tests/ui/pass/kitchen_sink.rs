//! Every macro feature at once; this must keep compiling.

use nestrs::prelude::*;

trait Repo: Send + Sync {}

#[injectable]
struct MemRepo;
impl Repo for MemRepo {}

#[injectable]
struct Cache {
    #[inject(default)]
    entries: std::sync::Mutex<Vec<u8>>,
}

#[injectable]
struct Service {
    _repo: Arc<dyn Repo>,
    _cache: Arc<Cache>,
}

#[injectable]
struct Gate;
impl Guard for Gate {
    async fn check(&self, _parts: &mut Parts) -> Result<(), HttpError> {
        Ok(())
    }
}

#[injectable]
struct Api {
    _service: Arc<Service>,
}

#[controller("/api")]
#[guard(Gate)]
impl Api {
    #[get]
    async fn list(&self) -> Json<Vec<u8>> {
        Json(Vec::new())
    }

    #[get("/{id}")]
    async fn one(&self, Path(_id): Path<u32>) -> StatusCode {
        StatusCode::OK
    }

    #[post]
    #[guard(Gate)]
    async fn create(&self) -> StatusCode {
        StatusCode::CREATED
    }

    /// Plain helpers in the same impl block are left alone.
    #[allow(dead_code)]
    fn helper(&self) {}
}

#[module(global, providers = [Cache, Gate], exports = [Cache, Gate])]
struct Shared;

#[module(
    imports = [Shared],
    providers = [bind!(dyn Repo => MemRepo), Service],
    controllers = [Api],
    exports = [dyn Repo],
)]
struct Feature;

#[module(imports = [Feature])]
struct Root;

fn main() {
    let _ = App::create(Root);
}
