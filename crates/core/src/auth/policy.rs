//! The access policy table: what each method of a service requires.
//!
//! `#[access_controlled_service]` implements [`ServicePolicy`] for the
//! generated server type (`FooServer<Impl>`), and `GrpcServer::add_service`
//! enforces it. Entries use Rust names (`get_end_point_one`); lookups also
//! accept wire names (`getEndPointOne`), ignoring case, `_` and `r#`.

use rust_auth_lib::keycloak::KeycloakClaims;

#[cfg(test)]
mod tests;

/// The access policy of every method `Self` implements for the generated
/// server `S`. Implemented by `#[access_controlled_service]`; do not implement
/// it by hand. `GrpcServer::add_service` requires it, so an unannotated type
/// cannot be served. Read it with [`PolicyTable::for_service`].
#[diagnostic::on_unimplemented(
    message = "`{Self}` has no access policy for the generated server `{S}`",
    label = "this type was not annotated for that service",
    note = "annotate the `impl` of the service trait with \
            `#[rust_grpc_lib::access_controlled_service]`, above `#[async_trait]`",
    note = "if the trait is imported by name and the generated server is not in scope, \
            add `server = path::to::FooServer` to the attribute"
)]
pub trait ServicePolicy<S> {
    /// One entry per method, in declaration order.
    const POLICIES: &'static [MethodPolicy];
}

/// How an [`Access::Roles`] policy combines its roles.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RoleMode {
    /// At least one of the roles must be held (`#[roles(any(..))]`).
    Any,
    /// Every role must be held (`#[roles(all(..))]`).
    All,
}

impl RoleMode {
    // `"any"` or `"all"`, as logged in the `role_mode` field of a denial.
    pub(crate) const fn as_str(self) -> &'static str {
        match self {
            Self::Any => "any",
            Self::All => "all",
        }
    }
}

/// What a method requires of its caller. Roles are Keycloak realm roles;
/// client roles never satisfy a policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Access {
    /// No token needed; one that is sent is not read (`#[public]`).
    Public,
    /// Any caller with a valid token (`#[any_authenticated]`).
    Authenticated,
    /// A valid token whose claims hold the listed realm roles
    /// (`#[roles(..)]`).
    Roles {
        /// How the roles combine.
        mode: RoleMode,
        /// The roles; never empty when built with [`Access::any_of`] or
        /// [`Access::all_of`].
        roles: &'static [&'static str],
    },
}

impl Access {
    /// The caller must hold at least one of `roles`.
    ///
    /// # Panics
    ///
    /// If `roles` is empty (a compile error in a constant).
    #[must_use = "this builds a policy value; it has no effect unless used"]
    pub const fn any_of(roles: &'static [&'static str]) -> Self {
        Self::with_mode(RoleMode::Any, roles)
    }

    /// The caller must hold every one of `roles`.
    ///
    /// # Panics
    ///
    /// If `roles` is empty (a compile error in a constant).
    #[must_use = "this builds a policy value; it has no effect unless used"]
    pub const fn all_of(roles: &'static [&'static str]) -> Self {
        Self::with_mode(RoleMode::All, roles)
    }

    // The empty-roles assertion is a security guard: an empty `All` would
    // admit every authenticated caller, and an empty `Any` would admit no one.
    const fn with_mode(mode: RoleMode, roles: &'static [&'static str]) -> Self {
        assert!(!roles.is_empty(), "a role policy needs at least one role");
        Self::Roles { mode, roles }
    }

    /// `"public"`, `"authenticated"` or `"roles"`: the `access` span field.
    #[must_use = "this only returns the name; it has no effect unless used"]
    pub const fn name(self) -> &'static str {
        match self {
            Self::Public => "public",
            Self::Authenticated => "authenticated",
            Self::Roles { .. } => "roles",
        }
    }

    // Whether `claims` satisfy this policy. Pass only validated claims.
    pub(crate) fn permits(self, claims: &KeycloakClaims) -> bool {
        match self {
            Self::Public | Self::Authenticated => true,
            Self::Roles {
                mode: RoleMode::Any,
                roles,
            } => roles.iter().any(|role| claims.has_role(role)),
            Self::Roles {
                mode: RoleMode::All,
                roles,
            } => roles.iter().all(|role| claims.has_role(role)),
        }
    }
}

/// The policy of one method, by its Rust name.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MethodPolicy {
    /// The method's name in the `impl` block, for example `get_end_point_one`.
    pub method: &'static str,
    /// What the method requires.
    pub access: Access,
}

impl MethodPolicy {
    /// Pair a Rust method name with its policy.
    #[must_use = "this builds a table entry; it has no effect unless used"]
    pub const fn new(method: &'static str, access: Access) -> Self {
        Self { method, access }
    }
}

/// A service's [`ServicePolicy`] table, searchable by method name, for
/// example to log the policies at startup. It borrows the constant table, so
/// it is `Copy` and lookups never allocate.
///
/// ```rust
/// # #[cfg(feature = "server")] {
/// use rust_grpc_lib::auth::{Access, MethodPolicy, PolicyTable, ServicePolicy};
/// # struct FooServer<T>(T);
/// # impl<T> FooServer<T> { fn new(inner: T) -> Self { Self(inner) } }
/// struct MyService;
/// // Normally emitted by `#[access_controlled_service]`.
/// impl ServicePolicy<FooServer<MyService>> for MyService {
///     const POLICIES: &'static [MethodPolicy] =
///         &[MethodPolicy::new("get_end_point_one", Access::any_of(&["operator"]))];
/// }
///
/// // The constructor names both types, as in `add_service`; it is not called.
/// let table = PolicyTable::for_service(FooServer::<MyService>::new);
/// assert_eq!(table.lookup("getEndPointOne"), Some(Access::any_of(&["operator"])));
/// assert_eq!(table.lookup("other"), None);
/// # }
/// ```
// Tables have a handful of entries, so a linear scan beats hashing.
#[derive(Debug, Clone, Copy)]
pub struct PolicyTable {
    policies: &'static [MethodPolicy],
}

impl PolicyTable {
    /// The table that `T` declares for the server `constructor` builds:
    /// `PolicyTable::for_service(FooServer::<MyFoo>::new)`.
    ///
    /// The constructor is never called; it only names the generated server
    /// `S` and the implementation `T`, as in
    /// `GrpcServerBuilder::add_service`. A type that serves two services has a
    /// table for each, chosen by the constructor.
    ///
    /// # Compile errors
    ///
    /// `T: ServicePolicy<S>` is unsatisfied if `T`'s `impl` block lacks
    /// `#[access_controlled_service]`, or if `constructor` belongs to a
    /// different service than that block.
    #[must_use = "the table is built, not stored anywhere; use it or drop the call"]
    pub fn for_service<T, S>(_constructor: impl FnOnce(T) -> S) -> Self
    where
        T: ServicePolicy<S>,
    {
        Self::of::<S, T>()
    }

    // The table `T` declares for the server `S`; the type-argument form that
    // `for_service` and `add_service` build on.
    pub(crate) fn of<S, T: ServicePolicy<S>>() -> Self {
        Self {
            policies: T::POLICIES,
        }
    }

    /// The policy for a method, by wire name (`getEndPointOne`) or Rust name
    /// (`get_end_point_one`); `None` (which the gate denies) if absent.
    #[must_use = "this only looks up a policy; it has no effect unless inspected"]
    pub fn lookup(&self, method: &str) -> Option<Access> {
        self.policies
            .iter()
            .find(|policy| same_method(policy.method, method))
            .map(|policy| policy.access)
    }

    /// Every entry, in declaration order.
    #[must_use = "this only returns the entries; it has no effect unless used"]
    pub fn policies(&self) -> impl Iterator<Item = &'static MethodPolicy> {
        self.policies.iter()
    }
}

// Whether two method names are equal once normalized. Must stay identical to
// `normalize` in `crates/grpc_macro/src/service.rs`, which rejects colliding
// names; a proc-macro crate cannot share this function.
pub(crate) fn same_method(a: &str, b: &str) -> bool {
    normalized(a).eq(normalized(b))
}

// tonic generates trait method names as the snake case of the proto names,
// so both sides are compared lowercase with underscores and any `r#` prefix
// removed: `getEndPointOne`, `GetBPMList` and `get_end_point_one` become
// `getendpointone`, `getbpmlist` and `getendpointone`.
fn normalized(name: &str) -> impl Iterator<Item = char> + '_ {
    name.trim_start_matches("r#")
        .chars()
        .filter(|c| *c != '_')
        .flat_map(char::to_lowercase)
}
