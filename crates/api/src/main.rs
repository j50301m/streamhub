use anyhow::Result;

pub mod extractors;
pub mod handlers;
mod initializer;
mod log_format;
pub mod middleware;
mod routes;
mod tasks;
pub mod ws;

#[cfg(test)]
mod tests;

#[tokio::main]
async fn main() -> Result<()> {
    let app = initializer::App::init().await?;
    app.run().await
}
