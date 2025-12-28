use crate::alloc_release_task::AllocationReleaseTasks;
use actix_web::web::{self, Data};
use actix_web::Scope;
use std::sync::Arc;
use ya_client_model::payment::PAYMENT_API_PATH;
use ya_persistence::executor::DbExecutor;
use ya_service_api_web::scope::ExtendableScope;
use ya_staking::StakingState;

mod accounts;
pub mod allocations;
mod debit_notes;
mod invoices;
mod payments;

mod batch;
mod cycle;
mod guard;
mod pay_activities;
mod pay_agreements;
pub mod staking;

pub fn api_scope(scope: Scope) -> Scope {
    scope
        .app_data(web::Data::new(guard::AgreementLock::arc()))
        .extend(accounts::register_endpoints)
        .extend(allocations::register_endpoints)
        .extend(debit_notes::register_endpoints)
        .extend(invoices::register_endpoints)
        .extend(payments::register_endpoints)
        .extend(pay_agreements::register_endpoints)
        .extend(pay_activities::register_endpoints)
        .extend(batch::register_endpoints)
        .extend(cycle::register_endpoints)
        .extend(staking::register_endpoints)
}

pub fn web_scope(
    db: &DbExecutor,
    allocation_release_tasks: AllocationReleaseTasks,
    staking: Option<Arc<StakingState>>,
) -> Scope {
    Scope::new(PAYMENT_API_PATH)
        .app_data(Data::new(db.clone()))
        .app_data(Data::new(allocation_release_tasks))
        .app_data(Data::new(staking))
        .service(api_scope(Scope::new("")))
    // TODO: TEST
    // Scope::new(PAYMENT_API_PATH).extend(api_scope).app_data(Data::new(db.clone()))
}
