use rattler_solve::PackageIdentifier;
use serde::{Deserialize, Serialize};

#[derive(Debug, Deserialize)]
pub struct SolveEnvironment {
    pub name: String,
    pub platform: String,
    pub specs: Vec<String>,
    pub virtual_packages: Vec<String>,
    pub channels: Vec<String>,
}

#[derive(Serialize)]
pub struct SolveEnvironmentOk {
    pub packages: Vec<PackageIdentifier>,
}
