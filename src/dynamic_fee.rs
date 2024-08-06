use reqwest;
use serde_json::{json, Value};

pub async fn get_priority_fee_estimate(
    dynamic_fee_rpc_url: &str,
    dynamic_fee_strategy: &str,
) -> Result<u64, Box<dyn std::error::Error>> {
    let client = reqwest::Client::new();

    let result_spec;

    if dynamic_fee_strategy == "helius" {
      result_spec = "helius"
    } else if dynamic_fee_strategy == "rpcpool" {
      result_spec = "rpcpool"
    } else {
      result_spec = "helius"
    }


    let body;

    if result_spec == "rpcpool" {
        // Use the improved priority fees API
        body = json!({
            "jsonrpc": "2.0",
            "id": "priority-fee-estimate",
            "method": "getRecentPrioritizationFees",
            "params": [
                ["oreV2ZymfyeXgNgBdqMkumTqqAprVqgBWQfoYkrtKWQ"],
                {
                    "percentile": 5000,
                }
            ]
        })
    } else {
        // Use the current implementation (Helius API)
        body = json!({
            "jsonrpc": "2.0",
            "id": "priority-fee-estimate",
            "method": "getPriorityFeeEstimate",
            "params": [{
                "accountKeys": ["oreV2ZymfyeXgNgBdqMkumTqqAprVqgBWQfoYkrtKWQ"],
                "options": {
                    "recommended": true
                }
            }]
        })
    };

    let response: Value = client.post(dynamic_fee_rpc_url)
        .json(&body)
        .send()
        .await?
        .json()
        .await?;

    let priority_fee = if result_spec == "rpcpool" {
        // Parse the improved priority fees API response
        response["result"]
            .as_array()
            .and_then(|arr| arr.last())
            .and_then(|last| last["prioritizationFee"].as_u64())
            .ok_or_else(|| format!("Failed to parse priority fee. Response: {:?}", response))?
    } else {
        // Parse the current implementation response
        response["result"]["priorityFeeEstimate"]
            .as_f64()
            .map(|fee| fee as u64)
            .ok_or_else(|| format!("Failed to parse priority fee. Response: {:?}", response))?
    };

    println!("Current dynamic priority fee: {} (via {})", priority_fee, result_spec);

    Ok(priority_fee)
}