use reqwest;
use serde_json::{json, Value};

pub async fn get_priority_fee_estimate(dynamic_fee_rpc_url: &str) -> Result<u64, Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();
    let helius_url = dynamic_fee_rpc_url;

    let body = json!({
        "jsonrpc": "2.0",
        "id": "helius-test",
        "method": "getPriorityFeeEstimate",
        "params": [{
            "accountKeys": ["oreV2ZymfyeXgNgBdqMkumTqqAprVqgBWQfoYkrtKWQ"],
            "options": {
                "recommended": true
            }
        }]
    });


    let response: Value = client.post(helius_url)
        .json(&body)
        .send()
        .await?
        .json()
        .await?;

    let priority_fee = response["result"]["priorityFeeEstimate"]
        .as_f64()
        .ok_or_else(|| format!("Failed to parse priority fee. Response: {:?}", response))?;

    Ok(priority_fee as u64)
}