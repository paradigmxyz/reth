/// `AccountStatus` represents the various states an account can be in after being loaded from the
/// database.
///
/// After account get loaded from database it can be in a lot of different states
/// while we execute multiple transaction and even blocks over account that is in memory.
/// This structure models all possible states that account can be in.
///
/// # Variants
///
/// - `LoadedNotExisting`: the account has been loaded but does not exist.
/// - `Loaded`: the account has been loaded and exists.
/// - `LoadedEmptyEIP161`: the account is loaded and empty, as per EIP-161.
/// - `InMemoryChange`: there are changes in the account that exist only in memory.
/// - `Changed`: the account has been modified.
/// - `Destroyed`: the account has been destroyed.
/// - `DestroyedChanged`: the account has been destroyed and then modified.
/// - `DestroyedAgain`: the account has been destroyed again.
#[derive(Clone, Copy, Default, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[cfg_attr(feature = "serde", derive(serde::Serialize, serde::Deserialize))]
pub enum AccountStatus {
    /// The account has been loaded but does not exist.
    #[default]
    LoadedNotExisting,
    /// The account has been loaded and exists.
    Loaded,
    /// The account is loaded and empty, as per EIP-161.
    LoadedEmptyEIP161,
    /// There are changes in the account that exist only in memory.
    InMemoryChange,
    /// The account has been modified.
    Changed,
    /// The account has been destroyed.
    Destroyed,
    /// The account has been destroyed and then modified.
    DestroyedChanged,
    /// The account has been destroyed again.
    DestroyedAgain,
}

impl AccountStatus {
    /// Account is not modified and just loaded from database.
    pub const fn is_not_modified(&self) -> bool {
        matches!(self, Self::LoadedNotExisting | Self::Loaded | Self::LoadedEmptyEIP161)
    }

    /// Account was destroyed by calling SELFDESTRUCT.
    /// This means that full account and storage are inside memory.
    pub const fn was_destroyed(&self) -> bool {
        matches!(self, Self::Destroyed | Self::DestroyedChanged | Self::DestroyedAgain)
    }

    /// This means storage is known, it can be newly created or storage got destroyed.
    pub const fn is_storage_known(&self) -> bool {
        matches!(
            self,
            Self::LoadedNotExisting |
                Self::InMemoryChange |
                Self::Destroyed |
                Self::DestroyedChanged |
                Self::DestroyedAgain
        )
    }

    /// Returns the next account status on creation.
    pub const fn on_created(&self) -> Self {
        match self {
            // If account was destroyed previously just copy new info to it.
            Self::DestroyedAgain
            | Self::Destroyed
            | Self::DestroyedChanged => Self::DestroyedChanged,
            // If account is loaded from db.
            Self::LoadedNotExisting
            // Loaded empty eip161 to creates is not possible as CREATE2 was added after EIP-161
            | Self::LoadedEmptyEIP161
            | Self::Loaded
            | Self::Changed
            | Self::InMemoryChange => {
                // If account is loaded and not empty this means that account has some balance.
                // This means that account cannot be created.
                // We are assuming that EVM did necessary checks before allowing account to be created.
                Self::InMemoryChange
            }
        }
    }

    /// Returns the next account status on change.
    pub const fn on_changed(&self, had_no_nonce_and_code: bool) -> Self {
        match self {
            // If the account was loaded as not existing, promote it to changed.
            // This account was likely created by a balance transfer.
            //
            // Change on empty account, should transfer storage if there is any.
            // There is possibility that there are storage entries inside db.
            // That storage is used in merkle tree calculation before state clear EIP.
            //
            // In-memory changes are preserved.
            Self::LoadedNotExisting | Self::LoadedEmptyEIP161 | Self::InMemoryChange => {
                Self::InMemoryChange
            }
            // The account was loaded as existing.
            Self::Loaded => {
                if had_no_nonce_and_code {
                    // Account is fully in memory
                    Self::InMemoryChange
                } else {
                    // Can be contract and some of storage slots can be present inside db.
                    Self::Changed
                }
            }

            // On change, the "changed" type account statuses are preserved.
            // Any checks for empty accounts are done outside of this fn.
            Self::Changed => Self::Changed,

            // If account is destroyed and then changed this means this is
            // balance transfer.
            Self::DestroyedChanged | Self::Destroyed | Self::DestroyedAgain => {
                Self::DestroyedChanged
            }
        }
    }

    /// Returns the next account status on selfdestruct.
    pub const fn on_selfdestructed(&self) -> Self {
        match self {
            // Non existing account can't be destroyed.
            Self::LoadedNotExisting => Self::LoadedNotExisting,
            // If account is created and selfdestructed in the same block, mark it as destroyed
            // again. Note: There is no big difference between Destroyed and
            // DestroyedAgain in this case, but was added for clarity.
            Self::DestroyedChanged | Self::DestroyedAgain | Self::Destroyed => Self::DestroyedAgain,

            // Transition to destroyed status.
            _ => Self::Destroyed,
        }
    }

    /// Transition to other state while preserving invariance of this state.
    ///
    /// It this account was Destroyed and other account is not:
    /// - We should mark extended account as destroyed too.
    /// - And as other account had some changes, extended account should be marked as
    ///   `DestroyedChanged`.
    ///
    /// If both account are not destroyed and if this account is in memory:
    /// - This means that extended account is in memory too.
    ///
    /// Otherwise, if both are destroyed or other is destroyed:
    /// - Sets other status to extended account.
    pub fn transition(&mut self, other: Self) {
        *self = match (self.was_destroyed(), other.was_destroyed()) {
            (true, false) => Self::DestroyedChanged,
            (false, false) if *self == Self::InMemoryChange => Self::InMemoryChange,
            _ => other,
        };
    }
}

#[cfg(test)]
mod tests {

    use super::*;

    #[test]
    fn test_account_status() {
        // Account not modified
        assert!(AccountStatus::Loaded.is_not_modified());
        assert!(AccountStatus::LoadedEmptyEIP161.is_not_modified());
        assert!(AccountStatus::LoadedNotExisting.is_not_modified());
        assert!(!AccountStatus::Changed.is_not_modified());
        assert!(!AccountStatus::InMemoryChange.is_not_modified());
        assert!(!AccountStatus::Destroyed.is_not_modified());
        assert!(!AccountStatus::DestroyedChanged.is_not_modified());
        assert!(!AccountStatus::DestroyedAgain.is_not_modified());

        // We know full storage
        assert!(!AccountStatus::LoadedEmptyEIP161.is_storage_known());
        assert!(AccountStatus::LoadedNotExisting.is_storage_known());
        assert!(AccountStatus::InMemoryChange.is_storage_known());
        assert!(AccountStatus::Destroyed.is_storage_known());
        assert!(AccountStatus::DestroyedChanged.is_storage_known());
        assert!(AccountStatus::DestroyedAgain.is_storage_known());
        assert!(!AccountStatus::Loaded.is_storage_known());
        assert!(!AccountStatus::Changed.is_storage_known());

        // Account was destroyed
        assert!(!AccountStatus::LoadedEmptyEIP161.was_destroyed());
        assert!(!AccountStatus::LoadedNotExisting.was_destroyed());
        assert!(!AccountStatus::InMemoryChange.was_destroyed());
        assert!(AccountStatus::Destroyed.was_destroyed());
        assert!(AccountStatus::DestroyedChanged.was_destroyed());
        assert!(AccountStatus::DestroyedAgain.was_destroyed());
        assert!(!AccountStatus::Loaded.was_destroyed());
    }

    #[test]
    fn test_on_created() {
        assert_eq!(AccountStatus::Destroyed.on_created(), AccountStatus::DestroyedChanged);
        assert_eq!(AccountStatus::DestroyedAgain.on_created(), AccountStatus::DestroyedChanged);
        assert_eq!(AccountStatus::DestroyedChanged.on_created(), AccountStatus::DestroyedChanged);

        assert_eq!(AccountStatus::LoadedNotExisting.on_created(), AccountStatus::InMemoryChange);
        assert_eq!(AccountStatus::Loaded.on_created(), AccountStatus::InMemoryChange);
        assert_eq!(AccountStatus::LoadedEmptyEIP161.on_created(), AccountStatus::InMemoryChange);
        assert_eq!(AccountStatus::Changed.on_created(), AccountStatus::InMemoryChange);
        assert_eq!(AccountStatus::InMemoryChange.on_created(), AccountStatus::InMemoryChange);
    }

    #[test]
    fn test_on_changed() {
        assert_eq!(
            AccountStatus::LoadedNotExisting.on_changed(true),
            AccountStatus::InMemoryChange
        );
        assert_eq!(
            AccountStatus::LoadedNotExisting.on_changed(false),
            AccountStatus::InMemoryChange
        );

        assert_eq!(
            AccountStatus::LoadedEmptyEIP161.on_changed(true),
            AccountStatus::InMemoryChange
        );
        assert_eq!(
            AccountStatus::LoadedEmptyEIP161.on_changed(false),
            AccountStatus::InMemoryChange
        );

        assert_eq!(AccountStatus::Loaded.on_changed(true), AccountStatus::InMemoryChange);
        assert_eq!(AccountStatus::Loaded.on_changed(false), AccountStatus::Changed);

        assert_eq!(AccountStatus::Changed.on_changed(true), AccountStatus::Changed);
        assert_eq!(AccountStatus::Changed.on_changed(false), AccountStatus::Changed);

        assert_eq!(AccountStatus::InMemoryChange.on_changed(true), AccountStatus::InMemoryChange);
        assert_eq!(AccountStatus::InMemoryChange.on_changed(false), AccountStatus::InMemoryChange);

        assert_eq!(
            AccountStatus::DestroyedChanged.on_changed(true),
            AccountStatus::DestroyedChanged
        );
        assert_eq!(
            AccountStatus::DestroyedChanged.on_changed(false),
            AccountStatus::DestroyedChanged
        );

        assert_eq!(AccountStatus::Destroyed.on_changed(true), AccountStatus::DestroyedChanged);
        assert_eq!(AccountStatus::Destroyed.on_changed(false), AccountStatus::DestroyedChanged);

        assert_eq!(AccountStatus::DestroyedAgain.on_changed(true), AccountStatus::DestroyedChanged);
        assert_eq!(
            AccountStatus::DestroyedAgain.on_changed(false),
            AccountStatus::DestroyedChanged
        );
    }

    #[test]
    fn test_on_selfdestructed() {
        assert_eq!(
            AccountStatus::LoadedNotExisting.on_selfdestructed(),
            AccountStatus::LoadedNotExisting
        );

        assert_eq!(
            AccountStatus::DestroyedChanged.on_selfdestructed(),
            AccountStatus::DestroyedAgain
        );
        assert_eq!(
            AccountStatus::DestroyedAgain.on_selfdestructed(),
            AccountStatus::DestroyedAgain
        );
        assert_eq!(AccountStatus::Destroyed.on_selfdestructed(), AccountStatus::DestroyedAgain);

        assert_eq!(AccountStatus::Loaded.on_selfdestructed(), AccountStatus::Destroyed);
        assert_eq!(AccountStatus::LoadedEmptyEIP161.on_selfdestructed(), AccountStatus::Destroyed);
        assert_eq!(AccountStatus::InMemoryChange.on_selfdestructed(), AccountStatus::Destroyed);
        assert_eq!(AccountStatus::Changed.on_selfdestructed(), AccountStatus::Destroyed);
    }

    #[test]
    fn test_transition() {
        let mut status = AccountStatus::Destroyed;
        status.transition(AccountStatus::Loaded);
        assert_eq!(status, AccountStatus::DestroyedChanged);

        let mut status = AccountStatus::DestroyedChanged;
        status.transition(AccountStatus::InMemoryChange);
        assert_eq!(status, AccountStatus::DestroyedChanged);

        let mut status = AccountStatus::DestroyedAgain;
        status.transition(AccountStatus::Changed);
        assert_eq!(status, AccountStatus::DestroyedChanged);

        let mut status = AccountStatus::InMemoryChange;
        status.transition(AccountStatus::Loaded);
        assert_eq!(status, AccountStatus::InMemoryChange);

        let mut status = AccountStatus::InMemoryChange;
        status.transition(AccountStatus::Changed);
        assert_eq!(status, AccountStatus::InMemoryChange);

        let mut status = AccountStatus::Loaded;
        status.transition(AccountStatus::Changed);
        assert_eq!(status, AccountStatus::Changed);

        let mut status = AccountStatus::LoadedNotExisting;
        status.transition(AccountStatus::InMemoryChange);
        assert_eq!(status, AccountStatus::InMemoryChange);

        let mut status = AccountStatus::LoadedEmptyEIP161;
        status.transition(AccountStatus::Loaded);
        assert_eq!(status, AccountStatus::Loaded);

        let mut status = AccountStatus::Destroyed;
        status.transition(AccountStatus::DestroyedChanged);
        assert_eq!(status, AccountStatus::DestroyedChanged);

        let mut status = AccountStatus::DestroyedAgain;
        status.transition(AccountStatus::Destroyed);
        assert_eq!(status, AccountStatus::Destroyed);

        let mut status = AccountStatus::Loaded;
        status.transition(AccountStatus::Destroyed);
        assert_eq!(status, AccountStatus::Destroyed);

        let mut status = AccountStatus::Changed;
        status.transition(AccountStatus::DestroyedAgain);
        assert_eq!(status, AccountStatus::DestroyedAgain);
    }
}
