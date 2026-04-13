export RUSTFLAGS := "-D warnings"

_default:
    @just --list

check:
    cargo fmt --all --check
    cargo clippy --all-targets --all-features -- -D warnings
    cargo test --all-features
    cargo build --release --workspace

check-docker:
    docker build -f deploy/app/Dockerfile.api -t streamhub-api:local .
    docker build -f deploy/web/Dockerfile.web -t streamhub-web:local .

up:
    docker network create streamhub 2>/dev/null || true
    docker volume create recordings 2>/dev/null || true
    docker volume create thumbnails 2>/dev/null || true
    docker compose -f deploy/infra/docker-compose.yml up -d
    docker compose -f deploy/app/docker-compose.yml up --build -d
    docker compose -f deploy/media/docker-compose.yml up --build -d
    docker compose -f deploy/web/docker-compose.yml up --build -d

up-obs:
    docker network create streamhub 2>/dev/null || true
    docker compose -f deploy/observability/docker-compose.yml up -d

down-obs:
    docker compose -f deploy/observability/docker-compose.yml down

up-all: up up-obs

down:
    docker compose -f deploy/web/docker-compose.yml down
    docker compose -f deploy/media/docker-compose.yml down
    docker compose -f deploy/app/docker-compose.yml down
    docker compose -f deploy/infra/docker-compose.yml down
    docker network rm streamhub 2>/dev/null || true

down-all: down down-obs

check-all: check check-docker
