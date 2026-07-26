//! This module contains all code relevant to Agent Predict within Ashide.
//!
//! Agent Predict attempts to predict the next action the user will take in Ashide.

pub(crate) mod generate_ai_input_suggestions;
pub(crate) mod generate_am_query_suggestions;
pub mod next_command_model;
// FeatureFlag::PredictAMQueries / terminal/input.rs 中
// `predict_am_queries_future_handle` 作为控制开关/句柄代号保留。
pub mod prompt_suggestions;
