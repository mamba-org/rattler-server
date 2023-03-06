//! Contains data transfer objects (DTOs) used as input and output of HTTP requests

use rattler_conda_types::RepoDataRecord;
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
    pub packages: Vec<RepoDataRecord>,
}

#[derive(Serialize)]
pub struct SolveEnvironmentErr<T: Serialize> {
    pub error_kind: String,
    pub message: Option<String>,
    pub additional_info: Option<T>,
}
