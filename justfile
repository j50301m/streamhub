export RUSTFLAGS := "-D warnings"

_default:
    @just --list

check:
    cargo fmt --all --check
    cargo clippy --all-targets --all-features -- -D warnings
    cargo test --all-features
    cargo build --release --workspace

check-docker:
    docker build -f deploy/services/Dockerfile.api -t streamhub-api:local .
    docker build -f deploy/services/Dockerfile.web -t streamhub-web:local .

check-all: check check-docker
