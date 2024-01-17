mod available_packages_cache;
mod cli;
mod dto;
mod error;
mod generic_cache;

use crate::cli::Args;
use crate::dto::{SolveEnvironment, SolveEnvironmentOk};
use crate::error::{response_from_error, ApiError, ParseError, ParseErrors, ValidationError};
use anyhow::Context;
use available_packages_cache::AvailablePackagesCache;
use axum::extract::State;
use axum::response::{IntoResponse, Response};
use axum::{routing::post, Json, Router};
use clap::Parser;
use cli::Solver;
use futures::{StreamExt, TryStreamExt};
use rattler_conda_types::{
    Channel, ChannelConfig, GenericVirtualPackage, MatchSpec, PackageName, PackageRecord, Platform,
    RepoDataRecord,
};
use rattler_solve::{libsolv_c, resolvo, SolverImpl, SolverTask};

use std::str::FromStr;
use std::sync::Arc;
use std::time::Duration;
use tracing::{span, Instrument, Level};
use tracing_subscriber::fmt::format::{format, FmtSpan};

struct AppState<Solver> {
    available_packages: AvailablePackagesCache,
    concurrent_repodata_downloads_per_request: usize,
    channel_config: ChannelConfig,
    solver: Solver,
}

/// Checks the `AvailablePackagesCache` every minute to remove outdated entries
async fn cache_gc_task(state: Arc<AppState<Solver>>) {
    let mut interval_timer = tokio::time::interval(Duration::from_secs(60));
    loop {
        interval_timer.tick().await;
        state.available_packages.gc();
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let args = Args::parse();

    // TODO: this is all right for prototyping, but we will want to use a different subscriber for
    // production
    let subscriber = tracing_subscriber::fmt()
        .event_format(format().pretty())
        .with_span_events(FmtSpan::CLOSE)
        .with_env_filter("rattler_server=trace")
        .finish();
    tracing::subscriber::set_global_default(subscriber)?;

    let state = Arc::new(state_from_args(&args));

    tokio::spawn(cache_gc_task(state.clone()));

    let app = app(state);

    let listener = tokio::net::TcpListener::bind(format!("127.0.0.1:{}", args.port))
        .await
        .unwrap();

    axum::serve(listener, app.into_make_service()).await?;

    Ok(())
}

fn state_from_args(args: &Args) -> AppState<Solver> {
    let cache_expiration = Duration::from_secs(args.repodata_cache_expiration_seconds);

    AppState {
        available_packages: AvailablePackagesCache::new(cache_expiration, args.cache_dir.clone()),
        concurrent_repodata_downloads_per_request: args.concurrent_repodata_downloads_per_request,
        channel_config: ChannelConfig::default(),
        solver: args.solver,
    }
}

fn app(state: Arc<AppState<Solver>>) -> Router {
    Router::new()
        .route("/solve", post(solve_environment))
        .with_state(state)
}

#[tracing::instrument(level = "info", skip(state))]
async fn solve_environment(
    State(state): State<Arc<AppState<Solver>>>,
    Json(payload): Json<SolveEnvironment>,
) -> Response {
    let result = solve_environment_inner(state, payload).await;
    match result {
        Ok(packages) => Json(SolveEnvironmentOk { packages }).into_response(),
        Err(e) => response_from_error(e),
    }
}

async fn solve_environment_inner(
    state: Arc<AppState<Solver>>,
    payload: SolveEnvironment,
) -> Result<Vec<RepoDataRecord>, ApiError> {
    let root_span = span!(Level::TRACE, "solve_environment");
    let _enter = root_span.enter();

    // Get match specs
    let mut matchspecs = Vec::with_capacity(payload.specs.len());
    let mut invalid_matchspecs = Vec::new();
    for spec in &payload.specs {
        match MatchSpec::from_str(spec) {
            Ok(spec) => matchspecs.push(spec),
            Err(e) => invalid_matchspecs.push(ParseError {
                input: spec.to_string(),
                error: e.to_string(),
            }),
        }
    }

    // Forbid invalid matchspecs
    if !invalid_matchspecs.is_empty() {
        return Err(ApiError::Validation(ValidationError::MatchSpecs(
            ParseErrors(invalid_matchspecs),
        )));
    }

    // Get the virtual packages
    let mut virtual_packages = Vec::with_capacity(payload.virtual_packages.len());
    for spec in &payload.virtual_packages {
        virtual_packages
            .push(parse_virtual_package(spec.as_str()).map_err(ValidationError::VirtualPackage)?);
    }

    // Parse channels
    let mut channels = Vec::new();
    let mut invalid_channels = Vec::new();
    for channel in &payload.channels {
        match Channel::from_str(channel, &state.channel_config) {
            Ok(c) => channels.push(c),
            Err(e) => invalid_channels.push(ParseError {
                input: channel.to_string(),
                error: e.to_string(),
            }),
        }
    }

    // Forbid invalid channels
    if !invalid_channels.is_empty() {
        return Err(ApiError::Validation(ValidationError::Channels(
            ParseErrors(invalid_channels),
        )));
    }

    // Each channel contains multiple subdirectories. Users can specify the subdirectories they want
    // to use when specifying their channels. If the user didn't specify the default subdirectories
    // we use defaults based on the current platform.
    let target_platform = match Platform::from_str(&payload.platform) {
        Ok(p) => p,
        Err(e) => {
            return Err(ApiError::Validation(ValidationError::Platform(
                ParseError {
                    input: payload.platform.to_string(),
                    error: e.to_string(),
                },
            )));
        }
    };

    let default_platforms = &[target_platform, Platform::NoArch];

    // The (channel, platform) combinations that have their own repodata.json
    let channels_and_platforms = channels.into_iter().flat_map(|channel| {
        let platforms = channel
            .platforms
            .as_ref()
            .map(|p| p.as_slice())
            .unwrap_or(default_platforms)
            .to_vec();

        platforms.into_iter().map(move |p| (channel.clone(), p))
    });

    // Get the available packages for each (channel, platform) combination
    let available_packages: Vec<_> = futures::stream::iter(channels_and_platforms)
        .map(|(channel, platform)| {
            let state = &state;
            async move { state.available_packages.get(&channel, platform).await }
        })
        .buffer_unordered(state.concurrent_repodata_downloads_per_request)
        .try_collect()
        .await?;

    // This call will block for hundreds of milliseconds, or longer
    let result = tokio::task::spawn_blocking(move || {
        let problem = SolverTask {
            available_packages: &available_packages,
            virtual_packages,
            specs: matchspecs,
            locked_packages: Vec::new(),
            pinned_packages: Vec::new(),
        };

        match state.solver {
            Solver::Resolvo => resolvo::Solver.solve(problem),
            Solver::Libsolvc => libsolv_c::Solver.solve(problem),
        }
    })
    .instrument(span!(Level::DEBUG, "solve"))
    .await
    .context("solver thread panicked")
    .map_err(ApiError::Internal)?;

    Ok(PackageRecord::sort_topologically(result?))
}

fn parse_virtual_package(virtual_package: &str) -> Result<GenericVirtualPackage, ParseError> {
    let mut split = virtual_package.split('=');

    // Can unwrap first because split will always return at least one element
    let name = split.next().unwrap().to_string();
    let version = split
        .next()
        .unwrap_or("0")
        .parse()
        .map_err(|e| ParseError {
            input: virtual_package.to_string(),
            error: format!("invalid version - {e}"),
        })?;
    let build_string = split.next().unwrap_or("0").to_string();

    if split.next().is_some() {
        return Err(ParseError {
            input: virtual_package.to_string(),
            error: "too many equals signs".to_string(),
        });
    }

    Ok(GenericVirtualPackage {
        name: PackageName::try_from(name).map_err(|e| ParseError {
            input: virtual_package.to_string(),
            error: e.to_string(),
        })?,
        version,
        build_string,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use axum::http;
    use axum::http::{header, Request, StatusCode};
    use mktemp::Temp;
    use mockito::{Mock, ServerGuard};
    use reqwest::Url;
    use tower::util::ServiceExt;

    async fn dummy_app() -> (ServerGuard, Router) {
        let temp_dir = Temp::new_dir().unwrap();
        let cache_dir = temp_dir.to_path_buf();
        let mut state = state_from_args(&Args {
            concurrent_repodata_downloads_per_request: 1,
            repodata_cache_expiration_seconds: u64::MAX,
            // The port is ignored during testing
            port: 0,
            cache_dir,
            solver: Solver::Resolvo,
        });

        let mock_channel_server = mockito::Server::new_async().await;
        state.channel_config = ChannelConfig {
            channel_alias: Url::parse(&mock_channel_server.url()).unwrap(),
        };

        (mock_channel_server, app(Arc::new(state)))
    }

    fn default_solve_body() -> SolveEnvironment {
        SolveEnvironment {
            name: Some("dummy".to_string()),
            platform: "linux-64".to_string(),
            specs: Vec::new(),
            channels: vec!["conda-forge".to_string()],
            virtual_packages: Vec::new(),
        }
    }

    async fn setup_repodata_mocks(mock_server: &mut ServerGuard) -> Vec<Mock> {
        let endpoint1 = mock_server
            .mock("GET", "/conda-forge/linux-64/repodata.json")
            .with_body(small_repodata_json())
            .create_async()
            .await;

        let endpoint2 = mock_server
            .mock("GET", "/conda-forge/noarch/repodata.json")
            .with_body(empty_repodata_json())
            .create_async()
            .await;

        vec![endpoint1, endpoint2]
    }

    async fn post_solve(app: Router, body: SolveEnvironment) -> Response {
        let json = Body::from(serde_json::to_vec(&body).unwrap());

        let request = Request::builder()
            .uri("/solve")
            .method(http::Method::POST)
            .header(header::CONTENT_TYPE, mime::APPLICATION_JSON.as_ref())
            .body(json)
            .unwrap();
        let response = app.oneshot(request).await.unwrap();

        response
    }

    async fn response_body(response: Response) -> String {
        let mut stream = response.into_body().into_data_stream();
        let mut data = String::new();
        while let Some(chunk) = stream.next().await {
            data.push_str(&String::from_utf8_lossy(&chunk.unwrap()));
        }
        data
    }

    #[tokio::test]
    async fn test_solve_invalid_platform() {
        let body = SolveEnvironment {
            platform: "asdfasdf".to_string(),
            ..default_solve_body()
        };

        let (_mock_channel_server, app) = dummy_app().await;
        let response = post_solve(app, body).await;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response_body(response).await;
        assert!(body.contains("asdfasdf"), "The response body did not mention the offending platform! See below for the full body:\n{body}");
    }

    #[tokio::test]
    async fn test_solve_channel_not_found() {
        let body = default_solve_body();
        let (_mock_channel_server, app) = dummy_app().await;
        let response = post_solve(app, body).await;

        assert_eq!(response.status(), StatusCode::BAD_REQUEST);
        let body = response_body(response).await;
        assert!(
            body.contains("unable to retrieve repodata.json"),
            "Unexpected response! See below for the full body:\n{body}"
        );
    }

    #[tokio::test]
    async fn test_solve_happy_path() {
        let (mut mock_channel_server, app) = dummy_app().await;
        let mock_endpoints = setup_repodata_mocks(&mut mock_channel_server).await;

        let body = SolveEnvironment {
            virtual_packages: vec!["__unix".to_string()],
            specs: vec!["foo".to_string(), "bar".to_string()],
            ..default_solve_body()
        };
        let response = post_solve(app, body).await;

        for endpoint in mock_endpoints {
            endpoint.assert_async().await;
        }

        assert_eq!(response.status(), StatusCode::OK);
        let body = response_body(response).await;
        let body: SolveEnvironmentOk = serde_json::from_str(&body).unwrap();

        let resolved_package_names: Vec<_> = body
            .packages
            .iter()
            .map(|p| p.package_record.name.as_normalized())
            .collect();
        assert_eq!(resolved_package_names, vec!["foo", "bar"]);
    }

    #[tokio::test]
    async fn test_solve_unsolvable() {
        let (mut mock_channel_server, app) = dummy_app().await;
        let mock_endpoints = setup_repodata_mocks(&mut mock_channel_server).await;

        // `bar` depends on `__unix`, but no virtual packages are provided
        let body = SolveEnvironment {
            specs: vec!["bar".to_string()],
            ..default_solve_body()
        };
        let response = post_solve(app, body).await;

        for endpoint in mock_endpoints {
            endpoint.assert_async().await;
        }

        assert_eq!(response.status(), StatusCode::CONFLICT);
        let body = response_body(response).await;
        assert!(
            body.contains("bar * cannot be installed because there are no viable options"),
            "Unexpected body!\n{body}"
        )
    }

    fn empty_repodata_json() -> String {
        r#"{
          "info": {
            "subdir": "linux-64"
          },
          "packages": {},
          "packages.conda": {},
          "repodata_version": 1
        }"#
        .to_string()
    }

    fn small_repodata_json() -> String {
        r#"{
          "info": {
            "subdir": "linux-64"
          },
          "packages": {
            "foo-3.0.2-py36h1af98f8_1.tar.bz2": {
              "build": "py36h1af98f8_1",
              "build_number": 1,
              "depends": [],
              "license": "MIT",
              "license_family": "MIT",
              "md5": "d65ab674acf3b7294ebacaec05fc5b54",
              "name": "foo",
              "sha256": "1154fceeb5c4ee9bb97d245713ac21eb1910237c724d2b7103747215663273c2",
              "size": 414494,
              "subdir": "linux-64",
              "timestamp": 1605110689658,
              "version": "3.0.2"
            },
            "bar-1.0-unix_py36h1af98f8_2.tar.bz2": {
              "build": "unix_py36h1af98f8_2",
              "build_number": 1,
              "depends": [
                "__unix"
              ],
              "license": "MIT",
              "license_family": "MIT",
              "md5": "bc13aa58e2092bcb0b97c561373d3905",
              "name": "bar",
              "sha256": "97ec377d2ad83dfef1194b7aa31b0c9076194e10d995a6e696c9d07dd782b14a",
              "size": 414494,
              "subdir": "linux-64",
              "timestamp": 1605110689658,
              "version": "1.2.3"
            }
          },
          "packages.conda": {},
          "repodata_version": 1
        }"#
        .to_string()
    }
}
