pub mod analyzer;
mod error;
mod nested_property_path;
pub mod packet2;
mod wowsreplay;

pub use error::*;
pub use strum;
pub use wowsreplay::*;

#[cfg(feature = "arc")]
pub type Rc<T> = std::sync::Arc<T>;

#[cfg(not(feature = "arc"))]
pub type Rc<T> = std::rc::Rc<T>;
