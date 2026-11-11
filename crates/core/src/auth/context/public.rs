//! [`PublicContext`]: the handler parameter of a `#[public]` method.

use std::ops::Deref;

use tonic::{Request, Status};

use super::CallInfo;
#[cfg(any(feature = "test-util", test))]
use super::RpcMethod;

#[cfg(test)]
mod tests;

/// The handler parameter of a `#[public]` method: the message and the
/// [`CallInfo`] it dereferences to (read-only).
///
/// It has no claims or token, even if the caller sent one: the gate does not
/// read it and strips the header. To know the caller, use
/// `#[any_authenticated]` and [`RequestContext`](super::RequestContext).
#[derive(Debug)]
pub struct PublicContext<T> {
    /// The request message (or request stream).
    pub message: T,
    info: CallInfo,
}

impl<T> PublicContext<T> {
    /// Build a context from a request. Only macro-generated code calls this.
    ///
    /// # Errors
    ///
    /// Fails closed with `Internal "authentication is not configured"` when
    /// the request has no [`RpcMethod`](super::RpcMethod), i.e. it bypassed
    /// the gate.
    #[doc(hidden)]
    pub fn try_from_request(request: Request<T>) -> Result<Self, Status> {
        let (info, message) = CallInfo::from_request(request)?;
        Ok(Self { message, info })
    }

    /// A context for unit tests (`test-util` feature): method
    /// `test.Service/Test`, empty metadata, no peer. Simulate more with
    /// [`with_rpc`](Self::with_rpc), [`with_remote_addr`](Self::with_remote_addr),
    /// [`with_metadata`](Self::with_metadata) and
    /// [`with_extension`](Self::with_extension).
    ///
    /// ```rust
    /// # #[cfg(feature = "test-util")] {
    /// # use rust_grpc_lib::auth::PublicContext;
    /// # use rust_grpc_lib::tonic::metadata::MetadataMap;
    /// let mut metadata = MetadataMap::new();
    /// metadata.insert("x-request-id", "abc".parse().unwrap());
    /// let ctx = PublicContext::for_test("ping").with_metadata(metadata);
    /// assert_eq!(ctx.metadata.get("x-request-id").unwrap(), "abc");
    /// assert_eq!(ctx.rpc.method, "Test");
    /// # }
    /// ```
    #[cfg(any(feature = "test-util", test))]
    pub fn for_test(message: T) -> Self {
        Self {
            message,
            info: CallInfo::for_test(),
        }
    }

    /// Replace the RPC method of a [`for_test`](Self::for_test) context
    /// (`test-util` feature).
    #[cfg(any(feature = "test-util", test))]
    #[must_use = "this returns the updated context; the original is consumed"]
    pub fn with_rpc(mut self, rpc: RpcMethod) -> Self {
        self.info.rpc = rpc;
        self
    }

    /// Set the peer address of a [`for_test`](Self::for_test) context
    /// (`test-util` feature).
    #[cfg(any(feature = "test-util", test))]
    #[must_use = "this returns the updated context; the original is consumed"]
    pub fn with_remote_addr(mut self, remote_addr: std::net::SocketAddr) -> Self {
        self.info.remote_addr = Some(remote_addr);
        self
    }

    /// Replace the metadata of a [`for_test`](Self::for_test) context
    /// (`test-util` feature).
    #[cfg(any(feature = "test-util", test))]
    #[must_use = "this returns the updated context; the original is consumed"]
    pub fn with_metadata(mut self, metadata: tonic::metadata::MetadataMap) -> Self {
        self.info.metadata = metadata;
        self
    }

    /// Add a request extension to a [`for_test`](Self::for_test) context
    /// (`test-util` feature), replacing one of the same type.
    #[cfg(any(feature = "test-util", test))]
    #[must_use = "this returns the updated context; the original is consumed"]
    pub fn with_extension<E: Clone + Send + Sync + 'static>(mut self, extension: E) -> Self {
        self.info.extensions.insert(extension);
        self
    }
}

impl<T> Deref for PublicContext<T> {
    type Target = CallInfo;

    fn deref(&self) -> &CallInfo {
        &self.info
    }
}
