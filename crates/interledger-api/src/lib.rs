#![recursion_limit = "128"]
#[macro_use]
extern crate tower_web;

use bytes::Bytes;
use futures::Future;
use interledger_http::{HttpAccount, HttpStore};
use interledger_packet::Address;
use interledger_router::RouterStore;
use interledger_service::{Account as AccountTrait, IncomingService, Username};
use interledger_service_util::{BalanceStore, ExchangeRateStore};
use interledger_settlement::{SettlementAccount, SettlementStore};
use serde::Serialize;
use std::str;
use tower_web::{net::ConnectionStream, Extract, Response, ServiceBuilder};

mod routes;
use self::routes::*;

pub(crate) mod client;

pub(crate) const BEARER_TOKEN_START: usize = 7;

pub trait NodeStore: Clone + Send + Sync + 'static {
    type Account: AccountTrait;

    fn insert_account(
        &self,
        account: AccountDetails,
    ) -> Box<dyn Future<Item = Self::Account, Error = ()> + Send>;

    fn delete_account(
        &self,
        id: <Self::Account as AccountTrait>::AccountId,
    ) -> Box<dyn Future<Item = Self::Account, Error = ()> + Send>;

    fn update_account(
        &self,
        id: <Self::Account as AccountTrait>::AccountId,
        account: AccountDetails,
    ) -> Box<dyn Future<Item = Self::Account, Error = ()> + Send>;

    fn modify_account_settings(
        &self,
        id: <Self::Account as AccountTrait>::AccountId,
        settings: AccountSettings,
    ) -> Box<dyn Future<Item = Self::Account, Error = ()> + Send>;

    // TODO limit the number of results and page through them
    fn get_all_accounts(&self) -> Box<dyn Future<Item = Vec<Self::Account>, Error = ()> + Send>;

    fn set_static_routes<R>(&self, routes: R) -> Box<dyn Future<Item = (), Error = ()> + Send>
    where
        R: IntoIterator<Item = (String, <Self::Account as AccountTrait>::AccountId)>;

    fn set_static_route(
        &self,
        prefix: String,
        account_id: <Self::Account as AccountTrait>::AccountId,
    ) -> Box<dyn Future<Item = (), Error = ()> + Send>;

    fn set_ilp_address(&self, ilp_address: Address); 
}

/// AccountSettings is a subset of the user parameters defined in
/// AccountDetails. Its purpose is to allow a user to modify certain of their
/// parameters which they may want to re-configure in the future, such as their
/// tokens (which act as passwords), their settlement frequency preferences, or
/// their HTTP/BTP endpoints, since they may change their network configuration.
#[derive(Debug, Extract, Response, Clone, Default)]
pub struct AccountSettings {
    pub http_incoming_token: Option<String>,
    pub btp_incoming_token: Option<String>,
    pub http_outgoing_token: Option<String>,
    pub btp_outgoing_token: Option<String>,
    pub http_endpoint: Option<String>,
    pub btp_uri: Option<String>,
    pub settle_threshold: Option<i64>,
    // Note that this is intentionally an unsigned integer because users should
    // not be able to set the settle_to value to be negative (meaning the node
    // would pre-fund with the user)
    pub settle_to: Option<u64>,
}

/// The Account type for the RedisStore.
#[derive(Debug, Extract, Response, Clone, Serialize)]
pub struct AccountDetails {
    pub configured_ilp_address: Option<Address>,
    pub username: Username,
    pub asset_code: String,
    pub asset_scale: u8,
    #[serde(default = "u64::max_value")]
    pub max_packet_amount: u64,
    pub min_balance: Option<i64>,
    pub http_endpoint: Option<String>,
    pub http_incoming_token: Option<String>,
    pub http_outgoing_token: Option<String>,
    pub btp_uri: Option<String>,
    pub btp_incoming_token: Option<String>,
    pub settle_threshold: Option<i64>,
    pub settle_to: Option<i64>,
    pub routing_relation: Option<String>,
    pub round_trip_time: Option<u32>,
    pub amount_per_minute_limit: Option<u64>,
    pub packets_per_minute_limit: Option<u32>,
    pub settlement_engine_url: Option<String>,
}

pub struct NodeApi<S, I> {
    store: S,
    admin_api_token: String,
    default_spsp_account: Option<Username>,
    incoming_handler: I,
    server_secret: Bytes,
}

impl<S, I, A> NodeApi<S, I>
where
    S: NodeStore<Account = A>
        + HttpStore<Account = A>
        + BalanceStore<Account = A>
        + SettlementStore<Account = A>
        + RouterStore
        + ExchangeRateStore,
    I: IncomingService<A> + Clone + Send + Sync + 'static,
    A: AccountTrait + HttpAccount + SettlementAccount + Serialize + Send + Sync + 'static,
{
    pub fn new(
        server_secret: Bytes,
        admin_api_token: String,
        store: S,
        incoming_handler: I,
    ) -> Self {
        NodeApi {
            store,
            admin_api_token,
            default_spsp_account: None,
            incoming_handler,
            server_secret,
        }
    }

    pub fn default_spsp_account(&mut self, username: Username) -> &mut Self {
        self.default_spsp_account = Some(username);
        self
    }

    pub fn serve<T>(&self, incoming: T) -> impl Future<Item = (), Error = ()>
    where
        T: ConnectionStream,
        T::Item: Send + 'static,
    {
        ServiceBuilder::new()
            .resource(IlpApi::new(
                self.store.clone(),
                self.incoming_handler.clone(),
            ))
            .resource({
                let mut spsp = SpspApi::new(
                    self.server_secret.clone(),
                    self.store.clone(),
                    self.incoming_handler.clone(),
                );
                if let Some(username) = &self.default_spsp_account {
                    spsp.default_spsp_account(username.clone());
                }
                spsp
            })
            .resource(AccountsApi::new(
                self.admin_api_token.clone(),
                self.store.clone(),
            ))
            .resource(SettingsApi::new(
                self.admin_api_token.clone(),
                self.store.clone(),
            ))
            .serve(incoming)
    }
}

