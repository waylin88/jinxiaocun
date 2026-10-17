use crate::{config::Config, db::Db};

#[derive(Clone)]
pub struct AppState {
    pub pool: Db,
    pub cfg: Config,
}