use actix_web::web::{Data, Json, Path};
use actix_web::{web, HttpResponse, Scope};
use serde::Deserialize;
use std::sync::Arc;
use ya_staking::StakingState;

#[derive(Deserialize, serde::Serialize, Debug)]
pub struct RegisterReq {
    pub provider_id: String,
    pub stake: f64,
}

#[derive(Deserialize, serde::Serialize, Debug)]
pub struct AmountReq {
    pub provider_id: String,
    pub amount: f64,
}

#[derive(Deserialize, serde::Serialize, Debug)]
pub struct SlashReq {
    pub provider_id: String,
    pub amount: f64,
    pub reason: Option<String>,
}

// --- Consensus Requests ---
#[derive(Deserialize, serde::Serialize, Debug)]
pub struct CommitReq {
    pub task_id: String,
    pub provider_id: String,
    pub commitment_hash: String,
}

#[derive(Deserialize, serde::Serialize, Debug)]
pub struct RevealReq {
    pub task_id: String,
    pub provider_id: String,
    pub result: String,
    pub salt: String,
}

#[derive(Deserialize, serde::Serialize, Debug)]
pub struct ChallengeReq {
    pub task_id: String,
    pub accuser_id: String,
    pub defendant_id: String,
    pub evidence: Option<String>,
}

#[derive(Deserialize, serde::Serialize, Debug)]
pub struct ResolveReq {
    pub dispute_id: i64,
    pub guilty: bool,
}

pub fn register_endpoints(scope: Scope) -> Scope {
    scope
        .route("/staking/register", web::post().to(register))
        .route("/staking/stake", web::post().to(stake))
        .route("/staking/reward", web::post().to(reward))
        .route("/staking/slash", web::post().to(slash))
        .route("/staking/withdraw", web::post().to(withdraw))
        .route("/staking/commit", web::post().to(commit))
        .route("/staking/reveal", web::post().to(reveal))
        .route("/staking/challenge", web::post().to(challenge))
        .route("/staking/resolve", web::post().to(resolve))
        .route("/staking/{provider_id}", web::get().to(get_provider))
        .route(
            "/staking/events/{provider_id}",
            web::get().to(get_events),
        )
}

async fn register(
    state: Data<Option<Arc<StakingState>>>,
    req: Json<RegisterReq>,
) -> HttpResponse {
    match state.get_ref() {
        Some(state) => match state.register(&req.provider_id, req.stake) {
            Ok(rec) => HttpResponse::Ok().json(rec),
            Err(e) => HttpResponse::InternalServerError().body(e.to_string()),
        },
        None => HttpResponse::ServiceUnavailable().body("Staking not initialized"),
    }
}

async fn stake(
    state: Data<Option<Arc<StakingState>>>,
    req: Json<AmountReq>,
) -> HttpResponse {
    match state.get_ref() {
        Some(state) => match state.stake(&req.provider_id, req.amount) {
            Ok(rec) => HttpResponse::Ok().json(rec),
            Err(e) => HttpResponse::InternalServerError().body(e.to_string()),
        },
        None => HttpResponse::ServiceUnavailable().body("Staking not initialized"),
    }
}

async fn reward(
    state: Data<Option<Arc<StakingState>>>,
    req: Json<AmountReq>,
) -> HttpResponse {
    match state.get_ref() {
        Some(state) => match state.reward(&req.provider_id, req.amount) {
            Ok(rec) => HttpResponse::Ok().json(rec),
            Err(e) => HttpResponse::InternalServerError().body(e.to_string()),
        },
        None => HttpResponse::ServiceUnavailable().body("Staking not initialized"),
    }
}

async fn slash(
    state: Data<Option<Arc<StakingState>>>,
    req: Json<SlashReq>,
) -> HttpResponse {
    match state.get_ref() {
        Some(state) => match state.slash(&req.provider_id, req.amount, req.reason.clone()) {
            Ok(rec) => HttpResponse::Ok().json(rec),
            Err(e) => HttpResponse::InternalServerError().body(e.to_string()),
        },
        None => HttpResponse::ServiceUnavailable().body("Staking not initialized"),
    }
}

async fn withdraw(
    state: Data<Option<Arc<StakingState>>>,
    req: Json<AmountReq>,
) -> HttpResponse {
    match state.get_ref() {
        Some(state) => match state.withdraw(&req.provider_id, req.amount) {
            Ok(rec) => HttpResponse::Ok().json(rec),
            Err(e) => HttpResponse::InternalServerError().body(e.to_string()),
        },
        None => HttpResponse::ServiceUnavailable().body("Staking not initialized"),
    }
}

async fn get_provider(
    state: Data<Option<Arc<StakingState>>>,
    path: Path<String>,
) -> HttpResponse {
    let pid = path.into_inner();
    match state.get_ref() {
        Some(state) => match state.get_provider(&pid) {
            Ok(Some(rec)) => HttpResponse::Ok().json(rec),
            Ok(None) => HttpResponse::NotFound().body("Provider not found"),
            Err(e) => HttpResponse::InternalServerError().body(e.to_string()),
        },
        None => HttpResponse::ServiceUnavailable().body("Staking not initialized"),
    }
}

async fn get_events(
    state: Data<Option<Arc<StakingState>>>,
    path: Path<String>,
) -> HttpResponse {
    let pid = path.into_inner();
    match state.get_ref() {
        Some(state) => match state.get_events(&pid) {
            Ok(events) => HttpResponse::Ok().json(events),
            Err(e) => HttpResponse::InternalServerError().body(e.to_string()),
        },
        None => HttpResponse::ServiceUnavailable().body("Staking not initialized"),
    }
}

// --- Consensus Handlers ---

async fn commit(
    state: Data<Option<Arc<StakingState>>>,
    req: Json<CommitReq>,
) -> HttpResponse {
    match state.get_ref() {
        Some(state) => match state.commit(&req.task_id, &req.provider_id, &req.commitment_hash) {
            Ok(_) => HttpResponse::Ok().body("Committed"),
            Err(e) => HttpResponse::InternalServerError().body(e.to_string()),
        },
        None => HttpResponse::ServiceUnavailable().body("Staking not initialized"),
    }
}

async fn reveal(
    state: Data<Option<Arc<StakingState>>>,
    req: Json<RevealReq>,
) -> HttpResponse {
    match state.get_ref() {
        Some(state) => match state.reveal(&req.task_id, &req.provider_id, &req.result, &req.salt) {
            Ok(valid) => {
                if valid {
                    HttpResponse::Ok().body("Reveal Valid")
                } else {
                    HttpResponse::BadRequest().body("Reveal Invalid")
                }
            },
            Err(e) => HttpResponse::InternalServerError().body(e.to_string()),
        },
        None => HttpResponse::ServiceUnavailable().body("Staking not initialized"),
    }
}

async fn challenge(
    state: Data<Option<Arc<StakingState>>>,
    req: Json<ChallengeReq>,
) -> HttpResponse {
    match state.get_ref() {
        Some(state) => match state.challenge(&req.task_id, &req.accuser_id, &req.defendant_id, req.evidence.clone()) {
            Ok(id) => HttpResponse::Ok().json(id),
            Err(e) => HttpResponse::InternalServerError().body(e.to_string()),
        },
        None => HttpResponse::ServiceUnavailable().body("Staking not initialized"),
    }
}

async fn resolve(
    state: Data<Option<Arc<StakingState>>>,
    req: Json<ResolveReq>,
) -> HttpResponse {
    match state.get_ref() {
        Some(state) => match state.resolve_dispute(req.dispute_id, req.guilty) {
            Ok(_) => HttpResponse::Ok().body("Resolved"),
            Err(e) => HttpResponse::InternalServerError().body(e.to_string()),
        },
        None => HttpResponse::ServiceUnavailable().body("Staking not initialized"),
    }
}
