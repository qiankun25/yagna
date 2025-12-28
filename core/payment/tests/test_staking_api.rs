use test_context::test_context;
use ya_framework_basic::async_drop::DroppableTestContext;
use ya_framework_basic::log::enable_logs;
use ya_framework_basic::temp_dir;
use ya_framework_mocks::net::MockNet;
use ya_framework_mocks::node::MockNode;
use ya_payment::api::staking::{AmountReq, RegisterReq, SlashReq};
use ya_staking::ProviderRecord;

#[cfg_attr(not(feature = "system-test"), ignore)]
#[test_context(DroppableTestContext)]
#[serial_test::serial]
async fn test_staking_api_flow(ctx: &mut DroppableTestContext) -> anyhow::Result<()> {
    enable_logs(false);

    let dir = temp_dir!("test_staking_api_flow")?;
    let dir = dir.path();

    let net = MockNet::new().bind();

    // Use full Payment module which includes StakingState initialization
    let node = MockNode::new(net, "node-1", dir)
        .with_identity()
        .with_payment(None); // This uses RealPayment which initializes StakingState
    node.bind_gsb().await?;
    node.start_server(ctx).await?;

    let appkey = node.get_identity()?.create_identity_key("provider").await?;
    let rest_url = node.rest_url();
    let client = awc::Client::new();
    let auth_header = format!("Bearer {}", appkey.key);

    let provider_id = "node-1-provider";

    // 1. Register
    log::info!("Registering provider...");
    let req = RegisterReq {
        provider_id: provider_id.to_string(),
        stake: 100.0,
    };
    let mut resp = client
        .post(format!("{}payment-api/v1/staking/register", rest_url))
        .insert_header(("Authorization", auth_header.as_str()))
        .send_json(&req)
        .await
        .map_err(|e| anyhow::anyhow!("Request failed: {}", e))?;
    
    if !resp.status().is_success() {
        let body = resp.body().await.unwrap_or_default();
        log::error!("Register failed: status={}, body={:?}", resp.status(), body);
        panic!("Register failed: status={}, body={:?}", resp.status(), body);
    }
    
    let rec: ProviderRecord = resp.json().await?;
    assert_eq!(rec.provider_id, provider_id);
    assert_eq!(rec.stake, 100.0);

    // 2. Stake more
    log::info!("Staking more...");
    let req = AmountReq {
        provider_id: provider_id.to_string(),
        amount: 50.0,
    };
    let mut resp = client
        .post(format!("{}payment-api/v1/staking/stake", rest_url))
        .insert_header(("Authorization", auth_header.as_str()))
        .send_json(&req)
        .await
        .map_err(|e| anyhow::anyhow!("Request failed: {}", e))?;
    assert!(resp.status().is_success());
    let rec: ProviderRecord = resp.json().await?;
    assert_eq!(rec.stake, 150.0);

    // 3. Reward (Manual)
    log::info!("Adding reward...");
    let req = AmountReq {
        provider_id: provider_id.to_string(),
        amount: 20.0,
    };
    let mut resp = client
        .post(format!("{}payment-api/v1/staking/reward", rest_url))
        .insert_header(("Authorization", auth_header.as_str()))
        .send_json(&req)
        .await
        .map_err(|e| anyhow::anyhow!("Request failed: {}", e))?;
    assert!(resp.status().is_success());
    let rec: ProviderRecord = resp.json().await?;
    assert_eq!(rec.rewards, 20.0);

    // 4. Slash
    log::info!("Slashing...");
    let req = SlashReq {
        provider_id: provider_id.to_string(),
        amount: 30.0,
        reason: Some("bad behavior".to_string()),
    };
    let mut resp = client
        .post(format!("{}payment-api/v1/staking/slash", rest_url))
        .insert_header(("Authorization", auth_header.as_str()))
        .send_json(&req)
        .await
        .map_err(|e| anyhow::anyhow!("Request failed: {}", e))?;
    assert!(resp.status().is_success());
    let rec: ProviderRecord = resp.json().await?;
    assert_eq!(rec.slashed, 30.0);
    // Stake should be reduced by 30
    assert_eq!(rec.stake, 120.0);

    // 5. Withdraw
    log::info!("Withdrawing...");
    let req = AmountReq {
        provider_id: provider_id.to_string(),
        amount: 10.0,
    };
    let mut resp = client
        .post(format!("{}payment-api/v1/staking/withdraw", rest_url))
        .insert_header(("Authorization", auth_header.as_str()))
        .send_json(&req)
        .await
        .map_err(|e| anyhow::anyhow!("Request failed: {}", e))?;
    assert!(resp.status().is_success());
    let rec: ProviderRecord = resp.json().await?;
    // Rewards reduced by 10
    assert_eq!(rec.rewards, 10.0);

    // 6. Get Provider
    log::info!("Getting provider info...");
    let mut resp = client
        .get(&format!("{}payment-api/v1/staking/{}", rest_url, provider_id))
        .insert_header(("Authorization", auth_header.as_str()))
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Request failed: {}", e))?;
    assert!(resp.status().is_success());
    let rec: ProviderRecord = resp.json().await?;
    assert_eq!(rec.provider_id, provider_id);
    assert_eq!(rec.stake, 120.0);
    assert_eq!(rec.rewards, 10.0);
    assert_eq!(rec.slashed, 30.0);

    Ok(())
}
