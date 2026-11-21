//! Python bindings for the pod: the Python module `mc683xx_bdm`
//! (PLAN.md, Host), a thin layer over `mc683xx_bdm`'s `PodTarget`, one
//! open pod across calls.
//!
//! The bindings build only with the `python` feature, which maturin
//! enables; without it the crate is its Python-free parts, so the
//! workspace's tests need no Python.

#![cfg_attr(not(feature = "python"), expect(dead_code, reason = "the bindings use these with `python`"))]

mod access;
mod error;
#[cfg(feature = "python")]
mod pod;
#[cfg(feature = "python")]
mod python;
#[cfg(feature = "python")]
mod types;
