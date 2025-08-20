pub mod resolvers;
pub mod rest;
pub mod websockets;

// Re-export commonly used items
pub use resolvers::LnurlResolver;
pub use rest as handlers; // For backward compatibility
pub use websockets::websocket_handler;
