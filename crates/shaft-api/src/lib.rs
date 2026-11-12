//! Client for the ngrok REST API plus a catalog of the resources Shaft shows.

pub mod catalog;
pub mod client;

pub use catalog::{cell, resource, Column, ItemAction, ResourceKind, RESOURCES};
pub use client::{ApiClient, ApiError, DEFAULT_BASE};
pub use reqwest::Method;
