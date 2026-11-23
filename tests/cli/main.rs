//! Tests that start the `triplelight` binary or its server as a child process (one test target, so the binary builds once).
#[cfg(unix)]
mod disk_cli;
mod parallel_cli;
#[cfg(all(unix, feature = "server"))]
mod server_auth;
