use std::path::PathBuf;

use clap::Parser;

#[derive(Parser)]
pub struct Args {
    /// The port at which the server should listen
    #[arg(short, default_value_t = 3000, env = "RATTLER_SERVER_PORT")]
    pub port: u16,

    /// The amount of concurrent downloads of repodata.json files, during a single request. JSON
    /// downloads are very CPU-intensive, because they require parsing huge JSON bodies.
    #[arg(
        short,
        default_value_t = 1,
        env = "RATTLER_SERVER_PORT_CONCURRENT_DOWNLOADS"
    )]
    pub concurrent_repodata_downloads_per_request: usize,

    /// The amount of seconds after which a cached repodata.json expires, defaults to 30 minutes.
    #[arg(short, default_value_t = 30 * 60, env = "RATTLER_SERVER_CACHE_EXPIRATION_SECONDS")]
    pub repodata_cache_expiration_seconds: u64,

    /// The directory to store cached repodata.json files in.
    #[arg(long, default_value = get_default_cache_dir().into_os_string(), env = "RATTLER_CACHE_DIR", value_hint = clap::ValueHint::DirPath)]
    pub cache_dir: PathBuf,

    /// The solver implementation to use.
    #[arg(long, value_enum, default_value_t, env = "RATTLER_SOLVER")]
    pub solver: Solver,
}

#[derive(Clone, clap::ValueEnum, Default, Copy)]
pub enum Solver {
    #[default]
    Resolvo,
    Libsolvc,
}

fn get_default_cache_dir() -> PathBuf {
    let mut path = dirs::cache_dir().unwrap();
    path.push("rattler");
    path
}
