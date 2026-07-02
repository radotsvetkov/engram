//! # Engram gateway - one door to every model
//!
//! All LLM and embedding access goes through [`gateway::Gateway`]. That single
//! chokepoint is what makes three things possible at once:
//!
//! - **Metering** - every call's tokens and cost are counted ([`gateway::Meter`]).
//! - **Taint enforcement** - an untrusted call has its secret-bearing context
//!   stripped before it reaches the model, half of breaking the prompt-injection →
//!   exfiltration chain.
//! - **Audit** - every call and embedding is written to the signed [`engram_core::Ledger`].
//!
//! Backends sit behind the [`provider::Provider`] trait, so the choice of Anthropic,
//! OpenAI, OpenRouter, or a local model is a constructor argument. The offline
//! [`provider::MockProvider`] makes the whole thing testable without credentials.

pub mod gateway;
pub mod provider;
pub mod types;

pub use gateway::{Call, Gateway, Meter, MeterSnapshot, Price};
pub use provider::{approx_tokens, GatewayError, MockProvider, Provider, ScriptedProvider, ARGS_ERROR_KEY};
pub use types::{Completion, CompletionRequest, Message, Role, ToolCall, ToolDef};

#[cfg(feature = "http")]
pub use provider::{AnthropicProvider, HttpProvider};
