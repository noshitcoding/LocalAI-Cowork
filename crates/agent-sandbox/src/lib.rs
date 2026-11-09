//! Trusted Kubernetes Agent Sandbox control plane and sandboxd transports.
//! No operation retries process execution after an ambiguous transport failure.
pub mod control;
pub mod runtime;
pub use control::{Configuration, ControlPlane, OwnedSandbox, PodIdentity, Resources, StopReceipt};
