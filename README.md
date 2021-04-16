# sc-distribution-rs

Smart contract used for distribution of ESDT tokens, in particular for MEX.

## The big picture

Works in combination with snapshots scripts. Those are supposed to set the
rewards for each user and each user is supposed to claim its rewards. The
SC expects its distributed token to have been issued and have already
been set LocalMint and LocalBurn roles for it.

## How it works

### Setting up community rewards

For setting up community rewards, the owner of the contract calls
setCommunityReward with the total_amount and unlock_epoch.
This operation is a GlobalOperation so startGlobalOperation needs
to be called.

### Setting up user rewards

For setting up user rewards, the owner of the contract calls 
setPerUserRewards with: unlock_epoch a vector (user_address, amount).
This operation is a GlobalOperation so startGlobalOperation needs
to be called. This operation can run out of gas when called with a
large vector. So it should be called multiple times with smaller
chunks. The contract does certain verifications, like the community
total amount should be greater or equal with the sum of all users
rewards set. Also it checks for duplicates in the arrays.
In case of human error, undo functions can be called in order
to revert last community reward (undoLastCommunityReward)
and user rewards (undoUserRewardsBetweenEpochs) between certain epochs.
These functions are also Global Operations.

### Claiming rewards

The user can claim its rewards by calling claimRewards. The rewards
will be calculated for the last maximum of 4 reward distributions.
Anything above that will become unclaimable. The owner of the
contract can call clearUnclaimableRewards in order to clear
the rewards accumulated and that are unclaimable. This function should
never run out of gas and should be called until it returns the value 0,
which is the amount of user rewards cleared in that specific transaction.
This function wil fail if a GlobalOperation is ongoing.

