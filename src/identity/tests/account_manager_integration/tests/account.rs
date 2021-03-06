// Copyright 2018 The Fuchsia Authors. All rights reserved.
// Use of this source code is governed by a BSD-style license that can be
// found in the LICENSE file.

use {
    anyhow::{anyhow, format_err, Error},
    fidl::endpoints::{create_endpoints, ClientEnd, ServerEnd},
    fidl_fuchsia_auth::AuthenticationContextProviderMarker,
    fidl_fuchsia_identity_account::{
        AccountManagerGetAccountRequest, AccountManagerMarker,
        AccountManagerProvisionNewAccountRequest, AccountManagerProxy, AccountMarker, AccountProxy,
        Error as ApiError, Lifetime,
    },
    fidl_fuchsia_logger::LogSinkMarker,
    fidl_fuchsia_stash::StoreMarker,
    fidl_fuchsia_sys::{EnvironmentMarker, LauncherMarker},
    fidl_fuchsia_tracing_provider::RegistryMarker,
    fuchsia_async::{DurationExt, TimeoutExt},
    fuchsia_component_test::{Capability, ChildOptions, RealmBuilder, RealmInstance, Ref, Route},
    fuchsia_zircon as zx,
    futures::prelude::*,
    std::ops::Deref,
};

/// Type alias for the LocalAccountId FIDL type
type LocalAccountId = u64;

const ALWAYS_SUCCEED_AUTH_MECHANISM_ID: &str =
    "fuchsia-pkg://fuchsia.com/dev_authenticator#meta/dev_authenticator_always_succeed.cmx";

const ALWAYS_FAIL_AUTHENTICATION_AUTH_MECHANISM_ID: &str = concat!(
    "fuchsia-pkg://fuchsia.com/dev_authenticator",
    "#meta/dev_authenticator_always_fail_authentication.cmx"
);

const ACCOUNT_MANAGER_URL: &'static str =
    "fuchsia-pkg://fuchsia.com/account_manager_integration_test#meta/account_manager_for_test.cmx";
const STASH_URL: &'static str = "#meta/stash.cm";

/// Maximum time between a lock request and when the account is locked
const LOCK_REQUEST_DURATION: zx::Duration = zx::Duration::from_seconds(5);

/// Calls provision_new_account on the supplied account_manager, returning an error on any
/// non-OK responses, or the account ID on success.
async fn provision_new_account(
    account_manager: &AccountManagerProxy,
    lifetime: Lifetime,
    auth_mechanism_id: Option<&str>,
) -> Result<LocalAccountId, Error> {
    account_manager
        .provision_new_account(AccountManagerProvisionNewAccountRequest {
            lifetime: Some(lifetime),
            auth_mechanism_id: auth_mechanism_id.map(|id| id.to_string()),
            ..AccountManagerProvisionNewAccountRequest::EMPTY
        })
        .await?
        .map_err(|error| format_err!("ProvisionNewAccount returned error: {:?}", error))
}

/// Convenience function to calls get_account on the supplied account_manager.
async fn get_account(
    account_manager: &AccountManagerProxy,
    account_id: u64,
    acp_client_end: ClientEnd<AuthenticationContextProviderMarker>,
    account_server_end: ServerEnd<AccountMarker>,
) -> Result<Result<(), ApiError>, fidl::Error> {
    account_manager
        .get_account(AccountManagerGetAccountRequest {
            id: Some(account_id),
            context_provider: Some(acp_client_end),
            account: Some(account_server_end),
            ..AccountManagerGetAccountRequest::EMPTY
        })
        .await
}

/// A proxy to an account manager running in a nested environment.
struct NestedAccountManagerProxy {
    /// Proxy to account manager.
    account_manager_proxy: AccountManagerProxy,

    /// The realm instance which the account manager is running in.
    /// Needs to be kept in scope to keep the realm alive.
    _realm_instance: RealmInstance,
}

impl Deref for NestedAccountManagerProxy {
    type Target = AccountManagerProxy;

    fn deref(&self) -> &AccountManagerProxy {
        &self.account_manager_proxy
    }
}

/// Start account manager in an isolated environment and return a proxy to it.
async fn create_account_manager() -> Result<NestedAccountManagerProxy, Error> {
    let builder = RealmBuilder::new().await?;
    let account_manager = builder
        .add_legacy_child("account_manager", ACCOUNT_MANAGER_URL, ChildOptions::new())
        .await?;
    let stash = builder.add_child("stash", STASH_URL, ChildOptions::new()).await?;
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<StoreMarker>())
                .from(&stash)
                .to(&account_manager),
        )
        .await?;
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<LogSinkMarker>())
                .from(Ref::parent())
                .to(&account_manager)
                .to(&stash),
        )
        .await?;
    builder
        .add_route(
            Route::new().capability(Capability::storage("data")).from(Ref::parent()).to(&stash),
        )
        .await?;
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<EnvironmentMarker>())
                .capability(Capability::protocol::<LauncherMarker>())
                .capability(Capability::protocol::<RegistryMarker>())
                .from(Ref::parent())
                .to(&account_manager),
        )
        .await?;
    builder
        .add_route(
            Route::new()
                .capability(Capability::protocol::<AccountManagerMarker>())
                .from(&account_manager)
                .to(Ref::parent()),
        )
        .await?;

    let instance = builder.build().await?;

    let account_manager_proxy =
        instance.root.connect_to_protocol_at_exposed_dir::<AccountManagerMarker>()?;

    Ok(NestedAccountManagerProxy { account_manager_proxy, _realm_instance: instance })
}

/// Locks an account and waits for the channel to close.
async fn lock_and_check(account: &AccountProxy) -> Result<(), Error> {
    account.lock().await?.map_err(|err| anyhow!("Lock failed: {:?}", err))?;
    account
        .take_event_stream()
        .for_each(|_| async move {}) // Drain
        .map(|_| Ok(())) // Completed drain results in ok
        .on_timeout(LOCK_REQUEST_DURATION.after_now(), || Err(anyhow!("Locking timeout exceeded")))
        .await
}

// TODO(jsankey): Work with ComponentFramework and cramertj@ to develop a nice Rust equivalent of
// the C++ TestWithEnvironment fixture to provide isolated environments for each test case.  We
// are currently creating a new environment for account manager to run in, but the tests
// themselves run in a single environment.
#[fuchsia_async::run_singlethreaded(test)]
async fn test_provision_new_account() -> Result<(), Error> {
    let account_manager = create_account_manager().await?;

    // Verify we initially have no accounts.
    assert_eq!(account_manager.get_account_ids().await?, vec![]);

    // Provision a new account.
    let account_1 = provision_new_account(&account_manager, Lifetime::Persistent, None).await?;
    assert_eq!(account_manager.get_account_ids().await?, vec![account_1]);

    // Provision a second new account and verify it has a different ID.
    let account_2 = provision_new_account(&account_manager, Lifetime::Persistent, None).await?;
    assert_ne!(account_1, account_2);

    // Provision account with an auth mechanism
    let account_3 = provision_new_account(
        &account_manager,
        Lifetime::Persistent,
        Some(ALWAYS_SUCCEED_AUTH_MECHANISM_ID),
    )
    .await?;

    let account_ids = account_manager.get_account_ids().await?;
    assert_eq!(account_ids.len(), 3);
    assert!(account_ids.contains(&account_1));
    assert!(account_ids.contains(&account_2));
    assert!(account_ids.contains(&account_3));

    Ok(())
}

#[fuchsia_async::run_singlethreaded(test)]
async fn test_provision_then_lock_then_unlock_account() -> Result<(), Error> {
    let account_manager = create_account_manager().await?;

    // Provision account with an auth mechanism that passes authentication
    let account_id = provision_new_account(
        &account_manager,
        Lifetime::Persistent,
        Some(ALWAYS_SUCCEED_AUTH_MECHANISM_ID),
    )
    .await?;
    let (acp_client_end, _) = create_endpoints()?;
    let (account_client_end, account_server_end) = create_endpoints()?;
    assert_eq!(
        get_account(&account_manager, account_id, acp_client_end, account_server_end).await?,
        Ok(())
    );
    let account_proxy = account_client_end.into_proxy()?;

    // Lock the account and ensure that it's locked
    lock_and_check(&account_proxy).await?;

    // Unlock the account and re-acquire a channel
    let (acp_client_end, _) = create_endpoints()?;
    let (account_client_end, account_server_end) = create_endpoints()?;
    assert_eq!(
        get_account(&account_manager, account_id, acp_client_end, account_server_end).await?,
        Ok(())
    );
    let account_proxy = account_client_end.into_proxy()?;
    assert_eq!(account_proxy.get_lifetime().await.unwrap(), Lifetime::Persistent);
    Ok(())
}

// TODO(fxbug.dev/103639): Enable these tests after account manager migrates to CFv2
async fn _test_unlock_account() -> Result<(), Error> {
    let account_manager = create_account_manager().await?;

    // Provision account with an auth mechanism that passes authentication
    let account_id = provision_new_account(
        &account_manager,
        Lifetime::Persistent,
        Some(ALWAYS_SUCCEED_AUTH_MECHANISM_ID),
    )
    .await?;

    // Restart the account manager, now the account should be locked
    std::mem::drop(account_manager);
    let account_manager = create_account_manager().await?;

    // Unlock the account and acquire a channel to it
    let (acp_client_end, _) = create_endpoints()?;
    let (account_client_end, account_server_end) = create_endpoints()?;
    assert_eq!(
        get_account(&account_manager, account_id, acp_client_end, account_server_end).await?,
        Ok(())
    );
    let account_proxy = account_client_end.into_proxy()?;
    assert_eq!(account_proxy.get_lifetime().await.unwrap(), Lifetime::Persistent);
    Ok(())
}

#[fuchsia_async::run_singlethreaded(test)]
async fn test_provision_then_lock_then_unlock_fail_authentication() -> Result<(), Error> {
    let account_manager = create_account_manager().await?;

    // Provision account with an auth mechanism that fails authentication
    let account_id = provision_new_account(
        &account_manager,
        Lifetime::Persistent,
        Some(ALWAYS_FAIL_AUTHENTICATION_AUTH_MECHANISM_ID),
    )
    .await?;
    let (acp_client_end, _) = create_endpoints()?;
    let (account_client_end, account_server_end) = create_endpoints()?;
    assert_eq!(
        get_account(&account_manager, account_id, acp_client_end, account_server_end).await?,
        Ok(())
    );
    let account_proxy = account_client_end.into_proxy()?;

    // Lock the account and ensure that it's locked
    lock_and_check(&account_proxy).await?;

    // Attempting to unlock the account fails with an authentication error
    let (acp_client_end, _) = create_endpoints()?;
    let (_, account_server_end) = create_endpoints()?;
    assert_eq!(
        get_account(&account_manager, account_id, acp_client_end, account_server_end).await?,
        Err(ApiError::FailedAuthentication)
    );
    Ok(())
}

// TODO(fxbug.dev/103639): Enable these tests after account manager migrates to CFv2
async fn _test_unlock_account_fail_authentication() -> Result<(), Error> {
    let account_manager = create_account_manager().await?;

    // Provision account with an auth mechanism that fails authentication
    let account_id = provision_new_account(
        &account_manager,
        Lifetime::Persistent,
        Some(ALWAYS_FAIL_AUTHENTICATION_AUTH_MECHANISM_ID),
    )
    .await?;

    // Restart the account manager, now the account should be locked
    std::mem::drop(account_manager);
    let account_manager = create_account_manager().await?;

    // Attempting to unlock the account fails with an authentication error
    let (acp_client_end, _) = create_endpoints()?;
    let (_, account_server_end) = create_endpoints()?;
    assert_eq!(
        get_account(&account_manager, account_id, acp_client_end, account_server_end).await?,
        Err(ApiError::FailedAuthentication)
    );
    Ok(())
}

// This represents two nearly identical tests, one with ephemeral and one with persistent accounts
async fn get_account_and_persona_helper(lifetime: Lifetime) -> Result<(), Error> {
    let account_manager = create_account_manager().await?;

    assert_eq!(account_manager.get_account_ids().await?, vec![]);

    let account_id = provision_new_account(&account_manager, lifetime, None).await?;
    // Connect a channel to the newly created account and verify it's usable.
    let (acp_client_end, _) = create_endpoints()?;
    let (account_client_end, account_server_end) = create_endpoints()?;
    assert_eq!(
        get_account(&account_manager, account_id, acp_client_end, account_server_end).await?,
        Ok(())
    );
    let account_proxy = account_client_end.into_proxy()?;
    let account_auth_state = account_proxy.get_auth_state().await?;
    assert_eq!(account_auth_state, Err(ApiError::UnsupportedOperation));

    // Cannot lock account not protected by an auth mechanism
    assert_eq!(account_proxy.lock().await?, Err(ApiError::FailedPrecondition));
    assert_eq!(account_proxy.get_lifetime().await?, lifetime);

    // Connect a channel to the account's default persona and verify it's usable.
    let (persona_client_end, persona_server_end) = create_endpoints()?;
    assert!(account_proxy.get_default_persona(persona_server_end).await?.is_ok());
    let persona_proxy = persona_client_end.into_proxy()?;
    let persona_auth_state = persona_proxy.get_auth_state().await?;
    assert_eq!(persona_auth_state, Err(ApiError::UnsupportedOperation));
    assert_eq!(persona_proxy.get_lifetime().await?, lifetime);

    Ok(())
}

#[fuchsia_async::run_singlethreaded(test)]
async fn test_get_persistent_account_and_persona() -> Result<(), Error> {
    get_account_and_persona_helper(Lifetime::Persistent).await
}

#[fuchsia_async::run_singlethreaded(test)]
async fn test_get_ephemeral_account_and_persona() -> Result<(), Error> {
    get_account_and_persona_helper(Lifetime::Ephemeral).await
}

#[fuchsia_async::run_singlethreaded(test)]
async fn test_account_deletion() -> Result<(), Error> {
    let account_manager = create_account_manager().await?;

    assert_eq!(account_manager.get_account_ids().await?, vec![]);

    let account_1 = provision_new_account(&account_manager, Lifetime::Persistent, None).await?;
    let account_2 = provision_new_account(&account_manager, Lifetime::Persistent, None).await?;
    let existing_accounts = account_manager.get_account_ids().await?;
    assert!(existing_accounts.contains(&account_1));
    assert!(existing_accounts.contains(&account_2));
    assert_eq!(existing_accounts.len(), 2);

    // Delete an account and verify it is removed.
    assert_eq!(account_manager.remove_account(account_1).await?, Ok(()));
    assert_eq!(account_manager.get_account_ids().await?, vec![account_2]);
    // Connecting to the deleted account should fail.
    let (acp_client_end, _acp_server_end) = create_endpoints()?;
    let (_account_client_end, account_server_end) = create_endpoints()?;
    assert_eq!(
        get_account(&account_manager, account_1, acp_client_end, account_server_end).await?,
        Err(ApiError::NotFound)
    );

    Ok(())
}

/// Ensure that an account manager created in a specific environment picks up the state of
/// previous instances that ran in that environment. Also check that some basic operations work on
/// accounts created in that previous lifetime.
// TODO(fxbug.dev/103639): Enable these tests after account manager migrates to CFv2
async fn _test_lifecycle() -> Result<(), Error> {
    let account_manager = create_account_manager().await?;

    assert_eq!(account_manager.get_account_ids().await?, vec![]);

    let account_1 = provision_new_account(&account_manager, Lifetime::Persistent, None).await?;
    let account_2 = provision_new_account(&account_manager, Lifetime::Persistent, None).await?;
    let account_3 = provision_new_account(&account_manager, Lifetime::Ephemeral, None).await?;

    let existing_accounts = account_manager.get_account_ids().await?;
    assert_eq!(existing_accounts.len(), 3);

    // Kill and restart account manager in the same environment
    std::mem::drop(account_manager);
    let account_manager = create_account_manager().await?;

    let existing_accounts = account_manager.get_account_ids().await?;
    assert_eq!(existing_accounts.len(), 2); // The ephemeral account was dropped

    // Make sure we can't retrieve the ephemeral account
    let (acp_client_end, _) = create_endpoints()?;
    let (_account_client_end, account_server_end) = create_endpoints()?;
    assert_eq!(
        get_account(&account_manager, account_3, acp_client_end, account_server_end).await?,
        Err(ApiError::NotFound)
    );
    // Retrieve a persistent account that was created in the earlier lifetime
    let (acp_client_end, _) = create_endpoints()?;
    let (_account_client_end, account_server_end) = create_endpoints()?;
    assert_eq!(
        get_account(&account_manager, account_1, acp_client_end, account_server_end).await?,
        Ok(())
    );

    // Delete an account and verify it is removed.
    assert_eq!(account_manager.remove_account(account_2).await?, Ok(()));
    assert_eq!(account_manager.get_account_ids().await?, vec![account_1]);

    Ok(())
}
