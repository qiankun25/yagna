use test_context::test_context;
use ya_framework_basic::async_drop::DroppableTestContext;
use ya_framework_basic::log::enable_logs;
use ya_framework_basic::temp_dir;
use ya_framework_mocks::net::MockNet;
use ya_framework_mocks::node::MockNode;
use ya_payment::api::staking::{RegisterReq, CommitReq, RevealReq, ChallengeReq, ResolveReq};
use ya_staking::ProviderRecord;
use sha2::{Sha256, Digest};

fn compute_hash(result: &str, salt: &str) -> String {
    let mut hasher = Sha256::new();
    hasher.update(format!("{}{}", result, salt));
    format!("{:x}", hasher.finalize())
}

#[cfg_attr(not(feature = "system-test"), ignore)]
#[test_context(DroppableTestContext)]
#[serial_test::serial]
async fn test_staking_consensus_flow(ctx: &mut DroppableTestContext) -> anyhow::Result<()> {
    enable_logs(false);

    let dir = temp_dir!("test_staking_consensus_flow")?;
    let dir = dir.path();

    let net = MockNet::new().bind();

    // Use full Payment module which includes StakingState initialization
    let node = MockNode::new(net, "node-consensus", dir)
        .with_identity()
        .with_payment(None);
    node.bind_gsb().await?;
    node.start_server(ctx).await?;

    let appkey = node.get_identity()?.create_identity_key("provider").await?;
    let rest_url = node.rest_url();
    let client = awc::Client::new();
    let auth_header = format!("Bearer {}", appkey.key);

    let p1 = "provider-1";
    let p2 = "provider-2";
    let p3 = "provider-3";
    let requestor = "requestor-1";
    let task_id = "task-xyz";

    // 1. Register Providers
    for pid in &[p1, p2, p3, requestor] {
        let req = RegisterReq {
            provider_id: pid.to_string(),
            stake: if pid == &requestor { 0.0 } else { 1000.0 },
        };
        let resp = client
            .post(format!("{}payment-api/v1/staking/register", rest_url))
            .insert_header(("Authorization", auth_header.as_str()))
            .send_json(&req)
            .await
            .map_err(|e| anyhow::anyhow!("Request failed: {}", e))?;
        assert!(resp.status().is_success());
    }

    // 2. Commit
    // P1 and P2 agree on "42"
    // P3 says "00"
    let salt1 = "salt-A";
    let salt2 = "salt-B";
    let salt3 = "salt-C";

    let hash1 = compute_hash("42", salt1);
    let hash2 = compute_hash("42", salt2);
    let hash3 = compute_hash("00", salt3);

    let commits = vec![
        (p1, hash1),
        (p2, hash2),
        (p3, hash3),
    ];

    for (pid, hash) in commits {
        let req = CommitReq {
            task_id: task_id.to_string(),
            provider_id: pid.to_string(),
            commitment_hash: hash,
        };
        let resp = client
            .post(format!("{}payment-api/v1/staking/commit", rest_url))
            .insert_header(("Authorization", auth_header.as_str()))
            .send_json(&req)
            .await
            .map_err(|e| anyhow::anyhow!("Request failed: {}", e))?;
        assert!(resp.status().is_success());
    }

    // 3. Reveal
    let reveals = vec![
        (p1, "42", salt1),
        (p2, "42", salt2),
        (p3, "00", salt3),
    ];

    for (pid, res, salt) in reveals {
        let req = RevealReq {
            task_id: task_id.to_string(),
            provider_id: pid.to_string(),
            result: res.to_string(),
            salt: salt.to_string(),
        };
        let resp = client
            .post(format!("{}payment-api/v1/staking/reveal", rest_url))
            .insert_header(("Authorization", auth_header.as_str()))
            .send_json(&req)
            .await
            .map_err(|e| anyhow::anyhow!("Request failed: {}", e))?;
        assert!(resp.status().is_success());
    }

    // 4. Challenge P3
    let req = ChallengeReq {
        task_id: task_id.to_string(),
        accuser_id: requestor.to_string(),
        defendant_id: p3.to_string(),
        evidence: Some("Result mismatch with majority".to_string()),
    };
    let mut resp = client
        .post(format!("{}payment-api/v1/staking/challenge", rest_url))
        .insert_header(("Authorization", auth_header.as_str()))
        .send_json(&req)
        .await
        .map_err(|e| anyhow::anyhow!("Request failed: {}", e))?;
    
    assert!(resp.status().is_success());
    let dispute_id: i64 = resp.json().await?;
    log::info!("Dispute created with ID: {}", dispute_id);

    // 5. Resolve Dispute (Guilty)
    let req = ResolveReq {
        dispute_id,
        guilty: true,
    };
    let mut resp = client
        .post(format!("{}payment-api/v1/staking/resolve", rest_url))
        .insert_header(("Authorization", auth_header.as_str()))
        .send_json(&req)
        .await
        .map_err(|e| anyhow::anyhow!("Request failed: {}", e))?;
    if !resp.status().is_success() {
        let body = resp.body().await.unwrap_or_default();
        log::error!("Resolve failed: status={}, body={:?}", resp.status(), body);
        panic!("Resolve failed: status={}, body={:?}", resp.status(), body);
    }
    assert!(resp.status().is_success());

    // 6. Verify Slash
    let mut resp = client
        .get(&format!("{}payment-api/v1/staking/{}", rest_url, p3))
        .insert_header(("Authorization", auth_header.as_str()))
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Request failed: {}", e))?;
    let rec: ProviderRecord = resp.json().await?;
    
    // Initial 1000, Slashed 100 -> 900 Stake
    assert_eq!(rec.stake, 900.0);
    assert_eq!(rec.slashed, 100.0);

    // 7. Verify Reward for Requestor
    // Requestor was not registered explicitly, but reward upserts
    // Note: upsert_provider_db sets stake=0 if new.
    let mut resp = client
        .get(&format!("{}payment-api/v1/staking/{}", rest_url, requestor))
        .insert_header(("Authorization", auth_header.as_str()))
        .send()
        .await
        .map_err(|e| anyhow::anyhow!("Request failed: {}", e))?;
    let rec: ProviderRecord = resp.json().await?;
    
    // Reward is 50% of 100 = 50.0
    assert_eq!(rec.rewards, 50.0);

    Ok(())
}
