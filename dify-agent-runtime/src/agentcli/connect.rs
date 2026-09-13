use super::client::new_stub_client;
use super::env::Environment;

pub async fn run_connect(env: &Environment, argv: &[String], json_output: bool) -> Result<(), String> {
    let client = new_stub_client(env)?;
    let resp = client.connect(argv, "{}").await?;
    if json_output {
        println!("{}", serde_json::to_string(&resp).unwrap());
    } else {
        println!("connected {}", resp.connection_id);
    }
    Ok(())
}
