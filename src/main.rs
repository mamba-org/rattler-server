mod available_packages_cache;
mod fetch;
mod generic_cache;
mod solve_environment;

use crate::solve_environment::SolveEnvironmentOk;
use axum::extract::State;
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{routing::post, Json, Router};
use rattler_conda_types::{Channel, ChannelConfig, GenericVirtualPackage, MatchSpec, Platform};
use rattler_solve::{LibsolvBackend, SolverBackend, SolverProblem};
use serde::Serialize;
use std::collections::HashSet;
use std::net::SocketAddr;
use available_packages_cache::AvailablePackagesCache;
use std::str::FromStr;
use std::sync::Arc;

#[derive(Serialize)]
struct SolveError<T: Serialize> {
    message: String,
    extra_info: T,
}

struct AppState {
    available_packages: AvailablePackagesCache,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let state = AppState {
        available_packages: AvailablePackagesCache::new(),
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

    let mut available_packages = Vec::new();
    let default_platforms = &[target_platform, Platform::NoArch];

    // TODO: do this in parallel
    for channel in channels {
        let platforms = channel
            .platforms
            .as_ref()
            .map(|p| p.as_slice())
            .unwrap_or(default_platforms);
        for &platform in platforms {
            let repo_data = state.available_packages.get(&channel, platform).await;

            match repo_data {
                Ok(repo_data) => available_packages.push(repo_data),
                Err(error) => return (StatusCode::BAD_REQUEST, error).into_response(),
            }
        }
    }

    let solve_start = std::time::Instant::now();

    // This call will block for hundreds of milliseconds, or longer
    let result = tokio::task::spawn_blocking(move || {
        let available_packages: Vec<_> = available_packages
            .iter()
            .map(|repodata| repodata.as_libsolv_repo_data())
            .collect();
        let problem = SolverProblem {
            available_packages: available_packages.into_iter(),
            virtual_packages,
            specs: matchspecs,
            locked_packages: Vec::new(),
            pinned_packages: Vec::new(),
        };

        LibsolvBackend.solve(problem)
    })
    .await
    .unwrap();

    let response = match result {
        Ok(operations) => {
            let installed = operations.into_iter().collect();

            Json(SolveEnvironmentOk {
                packages: installed,
            })
            .into_response()
        }
        Err(e) => (StatusCode::BAD_REQUEST, format!("Unable to solve: {e:?}")).into_response(),
    };

    let solve_end = std::time::Instant::now();
    println!("Solve: {} ms", (solve_end - solve_start).as_millis());

    response
}
