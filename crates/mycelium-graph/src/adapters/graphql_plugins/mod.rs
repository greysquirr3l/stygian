//! GraphQL target plugin implementations.
//!
//! Each file in this module implements [`GraphQlTargetPlugin`](crate::ports::graphql_plugin::GraphQlTargetPlugin)
//! for one specific GraphQL API target.
//!
//! # Available plugins
//!
//! | Module | Target | Env var |
//! |--------|--------|---------|
//! | [`jobber`] | Jobber field-service management | `JOBBER_ACCESS_TOKEN` |

pub mod jobber;
