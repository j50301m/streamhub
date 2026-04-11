use anyhow::Result;

pub mod extractors;
pub mod handlers;
mod initializer;
pub mod middleware;
mod routes;

#[cfg(test)]
mod tests;

#[tokio::main]
async fn main() -> Result<()> {
    let app = initializer::App::init().await?;
    app.run().await
}
