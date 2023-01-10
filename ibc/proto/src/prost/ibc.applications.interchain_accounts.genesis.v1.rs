/// GenesisState defines the interchain accounts genesis state
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct GenesisState {
    #[prost(message, optional, tag="1")]
    pub controller_genesis_state: ::core::option::Option<ControllerGenesisState>,
    #[prost(message, optional, tag="2")]
    pub host_genesis_state: ::core::option::Option<HostGenesisState>,
}
/// ControllerGenesisState defines the interchain accounts controller genesis state
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ControllerGenesisState {
    #[prost(message, repeated, tag="1")]
    pub active_channels: ::prost::alloc::vec::Vec<ActiveChannel>,
    #[prost(message, repeated, tag="2")]
    pub interchain_accounts: ::prost::alloc::vec::Vec<RegisteredInterchainAccount>,
    #[prost(string, repeated, tag="3")]
    pub ports: ::prost::alloc::vec::Vec<::prost::alloc::string::String>,
    #[prost(message, optional, tag="4")]
    pub params: ::core::option::Option<super::super::controller::v1::Params>,
}
/// HostGenesisState defines the interchain accounts host genesis state
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct HostGenesisState {
    #[prost(message, repeated, tag="1")]
    pub active_channels: ::prost::alloc::vec::Vec<ActiveChannel>,
    #[prost(message, repeated, tag="2")]
    pub interchain_accounts: ::prost::alloc::vec::Vec<RegisteredInterchainAccount>,
    #[prost(string, tag="3")]
    pub port: ::prost::alloc::string::String,
    #[prost(message, optional, tag="4")]
    pub params: ::core::option::Option<super::super::host::v1::Params>,
}
/// ActiveChannel contains a connection ID, port ID and associated active channel ID
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct ActiveChannel {
    #[prost(string, tag="1")]
    pub connection_id: ::prost::alloc::string::String,
    #[prost(string, tag="2")]
    pub port_id: ::prost::alloc::string::String,
    #[prost(string, tag="3")]
    pub channel_id: ::prost::alloc::string::String,
}
/// RegisteredInterchainAccount contains a connection ID, port ID and associated interchain account address
#[derive(Clone, PartialEq, ::prost::Message)]
pub struct RegisteredInterchainAccount {
    #[prost(string, tag="1")]
    pub connection_id: ::prost::alloc::string::String,
    #[prost(string, tag="2")]
    pub port_id: ::prost::alloc::string::String,
    #[prost(string, tag="3")]
    pub account_address: ::prost::alloc::string::String,
}
