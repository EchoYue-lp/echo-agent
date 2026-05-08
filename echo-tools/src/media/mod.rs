//! Media processing tools for images and documents.
//!
//! Provides tools for:
//! - Fetching images from URLs and converting to base64
//! - Enhanced web fetch with image support

pub mod image_fetch;
pub mod web_fetch_enhanced;

pub use image_fetch::ImageFetchTool;
pub use web_fetch_enhanced::WebFetchToolEnhanced;
