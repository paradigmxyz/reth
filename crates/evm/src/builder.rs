//! Builder for creating an EVM with a database and environment.

use alloc::boxed::Box;
use revm::{inspector_handle_register, Database, Evm, EvmBuilder, GetInspector};
use revm_primitives::EnvWithHandlerCfg;

/// Builder for creating an EVM with a database and environment.
///
/// Wrapper around [`EvmBuilder`] that allows for setting the database and environment for the EVM.
///
/// This is useful for creating an EVM with a custom database and environment without having to
/// necessarily rely on Revm inspector.
#[derive(Debug)]
pub struct RethEvmBuilder<DB: Database, EXT = ()> {
    /// The database to use for the EVM.
    db: DB,
    /// The environment to use for the EVM.
    env: Option<Box<EnvWithHandlerCfg>>,
    /// The external context for the EVM.
    external_context: EXT,
}

impl<DB, EXT> RethEvmBuilder<DB, EXT>
where
    DB: Database,
{
    /// Create a new EVM builder with the given database.
    pub const fn new(db: DB, external_context: EXT) -> Self {
        Self { db, env: None, external_context }
    }

    /// Set the environment for the EVM.
    pub fn with_env(mut self, env: Box<EnvWithHandlerCfg>) -> Self {
        self.env = Some(env);
        self
    }

    /// Set the external context for the EVM.
    pub fn with_external_context<EXT1>(self, external_context: EXT1) -> RethEvmBuilder<DB, EXT1> {
        RethEvmBuilder { db: self.db, env: self.env, external_context }
    }

    /// Build the EVM with the given database and environment.
    pub fn build<'a>(self) -> Evm<'a, EXT, DB> {
        let mut builder =
            EvmBuilder::default().with_db(self.db).with_external_context(self.external_context);
        if let Some(env) = self.env {
            builder = builder.with_spec_id(env.clone().spec_id());
            builder = builder.with_env(env.env);
        }

        builder.build()
    }

    /// Build the EVM with the given database and environment, using the given inspector.
    pub fn build_with_inspector<'a, I>(self, inspector: I) -> Evm<'a, I, DB>
    where
        I: GetInspector<DB>,
        EXT: 'a,
    {
        let mut builder =
            EvmBuilder::default().with_db(self.db).with_external_context(self.external_context);
        if let Some(env) = self.env {
            builder = builder.with_spec_id(env.clone().spec_id());
            builder = builder.with_env(env.env);
        }
        builder
            .with_external_context(inspector)
            .append_handler_register(inspector_handle_register)
            .build()
    }
}

/// Trait for configuring an EVM builder.
pub trait ConfigureEvmBuilder {
    /// The type of EVM builder that this trait can configure.
    type Builder<'a, DB: Database>: EvmFactory;
}

/// Trait for configuring the EVM for executing full blocks.
pub trait EvmFactory {
    /// Associated type for the default external context that should be configured for the EVM.
    type DefaultExternalContext<'a>;

    /// Provides the default external context.
    fn default_external_context<'a>(&self) -> Self::DefaultExternalContext<'a>;

    /// Returns new EVM with the given database
    ///
    /// This does not automatically configure the EVM with [`crate::ConfigureEvmEnv`] methods. It is
    /// up to the caller to call an appropriate method to fill the transaction and block
    /// environment before executing any transactions using the provided EVM.
    fn evm<DB: Database>(self, db: DB) -> Evm<'static, Self::DefaultExternalContext<'static>, DB>
    where
        Self: Sized,
    {
        RethEvmBuilder::new(db, self.default_external_context()).build()
    }

    /// Returns a new EVM with the given database configured with the given environment settings,
    /// including the spec id.
    ///
    /// This will preserve any handler modifications
    fn evm_with_env<'a, DB: Database + 'a>(
        &self,
        db: DB,
        env: EnvWithHandlerCfg,
    ) -> Evm<'a, Self::DefaultExternalContext<'a>, DB> {
        RethEvmBuilder::new(db, self.default_external_context()).with_env(env.into()).build()
    }

    /// Returns a new EVM with the given database configured with the given environment settings,
    /// including the spec id.
    ///
    /// This will use the given external inspector as the EVM external context.
    ///
    /// This will preserve any handler modifications
    fn evm_with_env_and_inspector<DB, I>(
        &self,
        db: DB,
        env: EnvWithHandlerCfg,
        inspector: I,
    ) -> Evm<'_, I, DB>
    where
        DB: Database,
        I: GetInspector<DB>,
    {
        RethEvmBuilder::new(db, self.default_external_context())
            .with_env(env.into())
            .build_with_inspector(inspector)
    }

    /// Returns a new EVM with the given inspector.
    ///
    /// Caution: This does not automatically configure the EVM with [`crate::ConfigureEvmEnv`]
    /// methods. It is up to the caller to call an appropriate method to fill the transaction
    /// and block environment before executing any transactions using the provided EVM.
    fn evm_with_inspector<DB, I>(&self, db: DB, inspector: I) -> Evm<'_, I, DB>
    where
        DB: Database,
        I: GetInspector<DB>,
    {
        RethEvmBuilder::new(db, self.default_external_context()).build_with_inspector(inspector)
    }
}

impl<DB: Database, EXT: Clone> EvmFactory for RethEvmBuilder<DB, EXT> {
    type DefaultExternalContext<'a> = EXT;

    fn default_external_context<'a>(&self) -> Self::DefaultExternalContext<'a> {
        self.external_context.clone()
    }
}
