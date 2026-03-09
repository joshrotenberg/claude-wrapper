//! Check CLI health: version, auth status, and doctor.
//!
//! ```sh
//! cargo run --example health_check
//! ```

use claude_wrapper::{AuthStatusCommand, Claude, ClaudeCommand, DoctorCommand, VersionCommand};

#[tokio::main]
async fn main() -> claude_wrapper::Result<()> {
    let claude = Claude::builder().build()?;

    // Version
    let version = VersionCommand::new().execute(&claude).await?;
    println!("Version: {}", version.stdout.trim());

    // Auth status
    match AuthStatusCommand::new().execute_json(&claude).await {
        Ok(status) => {
            println!("Authenticated: {}", status.authenticated);
            if let Some(email) = &status.email {
                println!("Email: {email}");
            }
        }
        Err(e) => println!("Auth check failed: {e}"),
    }

    // Doctor
    let doctor = DoctorCommand::new().execute(&claude).await?;
    println!("\nDoctor:\n{}", doctor.stdout);

    Ok(())
}
