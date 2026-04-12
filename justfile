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

docker-compose-up:
    docker compose -f deploy/services/docker-compose.yml down
    docker compose -f deploy/services/docker-compose.yml up --build api web -d


check-all: check check-docker
