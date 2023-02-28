mod solve_environment;

use crate::solve_environment::SolveEnvironmentOk;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{routing::post, Json, Router};
use futures::{StreamExt, TryFutureExt};
use rattler_conda_types::{
    Channel, ChannelConfig, GenericVirtualPackage, MatchSpec, Platform, RepoData,
};
use rattler_repodata_gateway::fetch::FetchRepoDataOptions;
use rattler_solve::{LibsolvBackend, SolverBackend, SolverProblem};
use reqwest::Client;
use serde::Serialize;
use std::collections::HashSet;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::str::FromStr;
use std::sync::Arc;

#[derive(Serialize)]
struct SolveError<T: Serialize> {
    message: String,
    extra_info: T,
}

struct AppState {
    repo_data_cache_path: PathBuf,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let repo_data_cache_path =
        PathBuf::from(std::env::var("RATTLER_SERVER_REPO_DATA_DIR").unwrap());
    let state = AppState {
        repo_data_cache_path,
    };

    let app = Router::new()
        .route("/solve", post(solve_environment))
        .with_state(Arc::new(state));

    let addr = SocketAddr::from(([127, 0, 0, 1], 3000));
    axum::Server::bind(&addr)
        .serve(app.into_make_service())
        .await?;

    Ok(())
}

async fn solve_environment(
    State(state): State<Arc<AppState>>,
    Json(payload): Json<solve_environment::SolveEnvironment>,
) -> Response {
    let channel_config = ChannelConfig::default();

    // Get match specs
    let mut matchspecs = Vec::with_capacity(payload.specs.len());
    let mut invalid_matchspecs = Vec::new();
    for spec in &payload.specs {
        match MatchSpec::from_str(spec, &channel_config) {
            Ok(spec) => matchspecs.push(spec),
            Err(e) => invalid_matchspecs.push((spec, e.to_string())),
        }
    }

    // Forbid invalid matchspecs
    if !invalid_matchspecs.is_empty() {
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
        let mut split = spec.split('=');

        // Can unwrap because split will always return at least one element
        let name = split.next().unwrap().to_string();

        // TODO: handle invalid version instead of panic!
        let version = split.next().unwrap_or("0").parse().unwrap();
        let build_string = split.next().unwrap_or("0").to_string();

        // TODO: proper parsing of virtual packages (this one allows invalid ones, like a=0=c=d)

        virtual_packages.push(GenericVirtualPackage {
            name,
            version,
            build_string,
        })
    }

    // Deduplicate channels, just to be sure
    let channels = payload
        .channels
        .iter()
        .map(|s| s.as_str())
        .collect::<HashSet<_>>();

    let channels = channels
        .into_iter()
        .map(|channel_str| Channel::from_str(channel_str, &channel_config))
        .collect::<Result<Vec<_>, _>>();

    let channels = match channels {
        Ok(c) => c,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(SolveError {
                    message: "Invalid channel".to_string(),
                    extra_info: e.to_string(),
                }),
            )
                .into_response()
        }
    };

    // Each channel contains multiple subdirectories. Users can specify the subdirectories they want
    // to use when specifying their channels. If the user didn't specify the default subdirectories
    // we use defaults based on the current platform.
    // TODO: let the user specify the platform
    let target_platform = match Platform::from_str(&payload.platform) {
        Ok(p) => p,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(SolveError {
                    message: format!("Invalid platform: {}", payload.platform),
                    extra_info: e.to_string(),
                }),
            )
                .into_response()
        }
    };

    let default_platforms = &[target_platform, Platform::NoArch];
    let channel_urls = channels
        .iter()
        .flat_map(|channel| {
            channel
                .platforms
                .as_ref()
                .map(|p| p.as_slice())
                .unwrap_or(default_platforms)
                .iter()
                .map(move |platform| (channel.clone(), *platform))
        })
        .collect::<Vec<_>>();

    // For each channel/subdirectory combination, download and cache the `repodata.json` that should
    // be available from the corresponding Url.
    let download_client = Client::builder()
        .no_gzip()
        .build()
        .expect("failed to create client");

    let repodata_cache_path = &state.repo_data_cache_path;
    let channel_and_platform_len = channel_urls.len();
    let repodata_download_client = download_client.clone();
    let available_packages = futures::stream::iter(channel_urls)
        .map(move |(channel, platform)| {
            let repodata_cache = repodata_cache_path.clone();
            let client = repodata_download_client.clone();

            async move {
                let result = rattler_repodata_gateway::fetch::fetch_repo_data(
                    channel.platform_url(platform),
                    client,
                    repodata_cache.as_path(),
                    FetchRepoDataOptions::default(),
                )
                .map_err(|e| e.to_string())
                .await?;

                // Deserialize the data. This is a hefty blocking operation so we spawn it as a tokio blocking
                // task.
                let repo_data_json_path = result.repo_data_json_path.clone();
                match tokio::task::spawn_blocking(move || {
                    RepoData::from_path(repo_data_json_path)
                        .map(move |repodata| repodata.into_repo_data_records(&channel))
                })
                .await
                {
                    Ok(Ok(repodata)) => Ok(repodata),
                    Ok(Err(err)) => Err(err.to_string()),
                    Err(err) => {
                        if let Ok(panic) = err.try_into_panic() {
                            std::panic::resume_unwind(panic);
                        }
                        // Since the task was cancelled most likely the whole async stack is being cancelled.
                        Ok(Vec::new())
                    }
                }
            }
        })
        .buffer_unordered(channel_and_platform_len)
        .collect::<Vec<_>>()
        .await
        // Collect into another iterator where we extract the first erroneous result
        .into_iter()
        .collect::<Result<Vec<_>, _>>();

    let available_packages = match available_packages {
        Ok(p) => p,
        Err(e) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(SolveError {
                    message: "Unable to retrieve available packages".to_string(),
                    extra_info: e,
                }),
            )
                .into_response();
        }
    };

    let problem = SolverProblem {
        available_packages,
        virtual_packages,
        specs: matchspecs,
        locked_packages: Vec::new(),
        pinned_packages: Vec::new(),
    };

    // TODO: this call will block for multiple seconds, we should run it on a separate thread!
    match LibsolvBackend.solve(problem) {
        Ok(operations) => {
            let installed = operations.into_iter().collect();

            Json(SolveEnvironmentOk {
                packages: installed,
            })
            .into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, format!("Unable to solve: {e:?}")).into_response(),
    }
}
