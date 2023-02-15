mod solve_environment;

use crate::solve_environment::SolveEnvironmentOk;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{routing::post, Json, Router};
use rattler_conda_types::{ChannelConfig, MatchSpec, RepoData};
use rattler_solve::{InstalledPackage, PackageOperationKind, RequestedAction, SolverProblem};
use serde::Serialize;
use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::Path;
use std::sync::Arc;

#[derive(Serialize)]
struct SolveError<T: Serialize> {
    message: String,
    extra_info: T,
}

struct AppState {
    repo_data: Vec<(String, RepoData)>,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    println!("Loading repo data");

    // Make sure to have this path set to the place where the `repodata.json` files can be found
    let conda_forge_path = std::env::var("CONDA_FORGE_REPODATA").unwrap();
    let linux64_repodata = Path::new(&conda_forge_path).join("linux-64/repodata.json");
    let noarch_repodata = Path::new(&conda_forge_path).join("noarch/repodata.json");

    let state = AppState {
        repo_data: vec![
            (
                "conda-forge".to_string(),
                serde_json::from_str(&std::fs::read_to_string(&linux64_repodata).unwrap()).unwrap(),
            ),
            (
                "conda-forge".to_string(),
                serde_json::from_str(&std::fs::read_to_string(&noarch_repodata).unwrap()).unwrap(),
            ),
        ],
    };

    println!("Repo data loaded!");

    let app = Router::new()
        .route("/solve", post(solve_environment))
        .with_state(Arc::new(state));

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await?;

    Ok(())
}

// TODO: this function will block for multiple seconds, we should run it on a separate thread!
async fn solve_environment(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<solve_environment::SolveEnvironment>,
) -> Response {
    // TODO: do we need a custom channel config?
    let channel_config = ChannelConfig::default();

    // Get match specs
    let mut matchspecs = Vec::with_capacity(payload.specs.len());
    let mut invalid_matchspecs = Vec::new();
    for spec in &payload.specs {
        match MatchSpec::from_str(&spec, &channel_config) {
            Ok(spec) => matchspecs.push((spec, RequestedAction::Install)),
            Err(e) => invalid_matchspecs.push((spec, e.to_string())),
        }
    }

    // Forbid invalid matchspecs
    if invalid_matchspecs.len() > 0 {
        let response = Json(SolveError {
            message: "Invalid matchspecs".to_string(),
            extra_info: invalid_matchspecs,
        });
        return (StatusCode::BAD_REQUEST, response).into_response();
    }

    // Get the virtual packages
    // TODO: do some kind of validation and forbid invalid ones
    let mut virtual_packages = Vec::with_capacity(payload.virtual_packages.len());
    for spec in &payload.virtual_packages {
        let mut split = spec.split("=");

        // Can unwrap because split will always return at least one element
        let name = split.next().unwrap().to_string();
        let version = split.next().unwrap_or("0");
        let build_string = split.next().map(|s| s.to_string());

        virtual_packages.push(InstalledPackage {
            name,
            version: version.to_string(),
            build_string,
            build_number: None,
        })
    }

    // Deduplicate channels, just to be sure
    let channels = payload
        .channels
        .iter()
        .map(|s| s.as_str())
        .collect::<HashSet<_>>();

    // Forbid unsupported channels
    if channels.len() > 0 && !channels.contains("conda-forge") {
        let response = Json(SolveError {
            message: "Unsupported channels".to_string(),
            extra_info: channels.into_iter().collect::<Vec<_>>(),
        });
        return (StatusCode::BAD_REQUEST, response).into_response();
    }

    // Forbid unsupported platforms
    if &payload.platform != "linux-64" {
        return (
            StatusCode::BAD_REQUEST,
            "Unsupported platform: only linux-64 is supported at the moment",
        )
            .into_response();
    }

    // Retrieve the necessary repo_data from the app state
    let channels: Vec<_> = state
        .repo_data
        .iter()
        .map(|(channel, repo_data)| (channel.to_string(), repo_data))
        .collect();

    // println!("Debug info:");
    // println!("Installed packages: {virtual_packages:?}");
    // println!("Specs: {matchspecs:?}");
    //
    // for (channel_name, repo_data) in &channels {
    //     println!("{} has {} packages", channel_name, repo_data.packages.len());
    // }

    let problem = SolverProblem {
        installed_packages: virtual_packages,
        specs: matchspecs,
        channels,
    };

    match problem.solve() {
        Ok(operations) => {
            let installed = operations
                .into_iter()
                .filter(|op| op.kind == PackageOperationKind::Install)
                .map(|op| op.package)
                .collect();

            Json(SolveEnvironmentOk {
                packages: installed,
            })
            .into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, format!("Unable to solve: {e:?}")).into_response(),
    }
}
