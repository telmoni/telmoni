//! `telmoni` — the server, and every job beside it, as one binary.
//!
//! ```text
//! telmoni serve                      listen on PORT (8082); every module, every loop
//! telmoni migrate                    every pending migration, then the grants; exit
//! telmoni rotate                     the audit partitions: create ahead, drop expired; exit
//! telmoni sweep <deletion|audit-verify|retention|agent-reindex>
//!                                    one sweep, once; exit
//! telmoni terminate <org_id>         an operator closes an organization
//! telmoni restore <org_id>           an operator brings one back
//! ```
//!
//! Every command reads the same environment (`.env` on a laptop). `serve`
//! and the sweeps need each module's DSN and the service secret; `migrate`
//! and `rotate` need the migrator's. The commands themselves are
//! [`telmoni::run`]; this binary hands it the process as the environment
//! describes it. A deployment's own binary hands it its own.

use telmoni::App;
use telmoni::cli::Command;

// The image is a static musl build, whose malloc serializes threads on one lock.
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let _ = dotenvy::dotenv();

    telmoni_shared::logging::init().map_err(|e| anyhow::anyhow!("logging: {e}"))?;

    let args: Vec<String> = std::env::args().skip(1).collect();
    let command = Command::parse(&args)?;
    telmoni::run(command, App::from_env).await
}
