use super::*;
use frame_support::{ pallet_prelude::DispatchResult};
use sp_std::convert::TryInto;
use sp_core::{H256, U256};
use crate::system::ensure_root;
use sp_io::hashing::sha2_256;
use sp_io::hashing::keccak_256;
use frame_system::{ensure_signed};
use sp_std::vec::Vec;
use substrate_fixed::types::I32F32;
use sp_runtime::{MultiAddress, traits::Verify};

const LOG_TARGET: &'static str = "runtime::subtensor::registration";

impl<T: Config> Pallet<T> {

    pub fn do_sudo_registration( 
        origin: T::RuntimeOrigin,
        netuid: u16, 
        hotkey: T::AccountId, 
        coldkey: T::AccountId,
        stake: u64,
        balance: u64,
    ) -> DispatchResult {
        ensure_root( origin )?;        
        ensure!( Self::if_subnet_exist( netuid ), Error::<T>::NetworkDoesNotExist ); 
        ensure!( !Uids::<T>::contains_key( netuid, &hotkey ), Error::<T>::AlreadyRegistered );

        Self::create_account_if_non_existent( &coldkey, &hotkey);         
        ensure!( Self::coldkey_owns_hotkey( &coldkey, &hotkey ), Error::<T>::NonAssociatedColdKey );
        Self::increase_stake_on_coldkey_hotkey_account( &coldkey, &hotkey, stake );

        let balance_to_be_added_as_balance = Self::u64_to_balance( balance );
        ensure!( balance_to_be_added_as_balance.is_some(), Error::<T>::CouldNotConvertToBalance );
        Self::add_balance_to_coldkey_account( &coldkey, balance_to_be_added_as_balance.unwrap() );

        let subnetwork_uid: u16;
        let current_block_number: u64 = Self::get_current_block_as_u64();
        let current_subnetwork_n: u16 = Self::get_subnetwork_n( netuid );
        if current_subnetwork_n < Self::get_max_allowed_uids( netuid ) {
            // --- 12.1.1 No replacement required, the uid appends the subnetwork.
            // We increment the subnetwork count here but not below.
            subnetwork_uid = current_subnetwork_n;

            // --- 12.1.2 Expand subnetwork with new account.
            Self::append_neuron( netuid, &hotkey, current_block_number );
            log::info!("add new neuron account");

        } else {
            // --- 12.1.1 Replacement required.
            // We take the neuron with the lowest pruning score here.
            subnetwork_uid = Self::get_neuron_to_prune( netuid );

            // --- 12.1.1 Replace the neuron account with the new info.
            Self::replace_neuron( netuid, subnetwork_uid, &hotkey, current_block_number );
            log::info!("prune neuron");
        }
    
        log::info!("NeuronRegistered( netuid:{:?} uid:{:?} hotkey:{:?}  ) ", netuid, subnetwork_uid, hotkey );
        Self::deposit_event( Event::NeuronRegistered( netuid, subnetwork_uid, hotkey ) );
        Ok(())
    }

    // ---- The implementation for the extrinsic do_burned_registration: registering by burning TAO.
    //
    // # Args:
    // 	* 'origin': (<T as frame_system::Config>RuntimeOrigin):
    // 		- The signature of the calling coldkey. 
    //             Burned registers can only be created by the coldkey.
    //
    // 	* 'netuid' (u16):
    // 		- The u16 network identifier.
    // 
    // 	* 'hotkey' ( T::AccountId ):
    // 		- Hotkey to be registered to the network.
    //   
    // # Event:
    // 	* NeuronRegistered;
    // 		- On successfully registereing a uid to a neuron slot on a subnetwork.
    //
    // # Raises:
    // 	* 'NetworkDoesNotExist':
    // 		- Attempting to registed to a non existent network.
    //
    // 	* 'TooManyRegistrationsThisBlock':
    // 		- This registration exceeds the total allowed on this network this block.
    //
    // 	* 'AlreadyRegistered':
    // 		- The hotkey is already registered on this network.
    //
    pub fn do_burned_registration( 
        origin: T::RuntimeOrigin,
        netuid: u16, 
        hotkey: T::AccountId, 
    ) -> DispatchResult {

        // --- 1. Check that the caller has signed the transaction. (the coldkey of the pairing)
        let coldkey = ensure_signed( origin )?; 
        log::info!("do_registration( coldkey:{:?} netuid:{:?} hotkey:{:?} )", coldkey, netuid, hotkey );

        // --- 2. Ensure the passed network is valid.
        ensure!( Self::if_subnet_exist( netuid ), Error::<T>::NetworkDoesNotExist ); 

        // --- 3. Ensure we are not exceeding the max allowed registrations per block.
        ensure!( Self::get_registrations_this_block( netuid ) < Self::get_max_registrations_per_block( netuid ), Error::<T>::TooManyRegistrationsThisBlock );

		// --- 4. Ensure we are not exceeding the max allowed registrations per interval.
		ensure!( Self::get_registrations_this_interval( netuid ) < Self::get_target_registrations_per_interval( netuid ) * 3 , Error::<T>::TooManyRegistrationsThisInterval );

        // --- 4. Ensure that the key is not already registered.
        ensure!( !Uids::<T>::contains_key( netuid, &hotkey ), Error::<T>::AlreadyRegistered );

        // --- 5. Ensure that the key passes the registration requirement
        ensure!( Self::passes_network_connection_requirement( netuid, &hotkey ), Error::<T>::DidNotPassConnectedNetworkRequirement );
    
        // --- 6. Ensure the callers coldkey has enough stake to perform the transaction.
        let current_block_number: u64 = Self::get_current_block_as_u64();
        let registration_cost_as_u64 = Self::get_burn_as_u64( netuid );
        let registration_cost_as_balance = Self::u64_to_balance( registration_cost_as_u64 ).unwrap();
        ensure!( Self::can_remove_balance_from_coldkey_account( &coldkey, registration_cost_as_balance ), Error::<T>::NotEnoughBalanceToStake );

        // --- 7. Ensure the remove operation from the coldkey is a success.
        ensure!( Self::remove_balance_from_coldkey_account( &coldkey, registration_cost_as_balance ) == true, Error::<T>::BalanceWithdrawalError );
        
        // The burn occurs here.
        TotalIssuance::<T>::put( TotalIssuance::<T>::get().saturating_sub( Self::get_burn_as_u64( netuid ) ) );

        // --- 8. If the network account does not exist we will create it here.
        Self::create_account_if_non_existent( &coldkey, &hotkey);         

        // --- 9. Ensure that the pairing is correct.
        ensure!( Self::coldkey_owns_hotkey( &coldkey, &hotkey ), Error::<T>::NonAssociatedColdKey );

        // --- 10. Append neuron or prune it.
        let subnetwork_uid: u16;
        let current_subnetwork_n: u16 = Self::get_subnetwork_n( netuid );

        // Possibly there is no neuron slots at all.
        ensure!( Self::get_max_allowed_uids( netuid ) != 0, Error::<T>::NetworkDoesNotExist );
        
        if current_subnetwork_n < Self::get_max_allowed_uids( netuid ) {
            // --- 11.1.1 No replacement required, the uid appends the subnetwork.
            // We increment the subnetwork count here but not below.
            subnetwork_uid = current_subnetwork_n;

            // --- 11.1.2 Expand subnetwork with new account.
            Self::append_neuron( netuid, &hotkey, current_block_number );
            log::info!("add new neuron account");
        } else {
            // --- 12.1.1 Replacement required.
            // We take the neuron with the lowest pruning score here.
            subnetwork_uid = Self::get_neuron_to_prune( netuid );

            // --- 12.1.1 Replace the neuron account with the new info.
            Self::replace_neuron( netuid, subnetwork_uid, &hotkey, current_block_number );
            log::info!("prune neuron");
        }

        // --- 13. Record the registration and increment block and interval counters.
        BurnRegistrationsThisInterval::<T>::mutate( netuid, |val| *val += 1 );
        RegistrationsThisInterval::<T>::mutate( netuid, |val| *val += 1 );
        RegistrationsThisBlock::<T>::mutate( netuid, |val| *val += 1 );
        Self::increase_rao_recycled( netuid, Self::get_burn_as_u64( netuid ) );
    
        // --- 14. Deposit successful event.
        log::info!("NeuronRegistered( netuid:{:?} uid:{:?} hotkey:{:?}  ) ", netuid, subnetwork_uid, hotkey );
        Self::deposit_event( Event::NeuronRegistered( netuid, subnetwork_uid, hotkey ) );

        // --- 15. Ok and done.
        Ok(())
    }

    // ---- The implementation for the extrinsic associate: associate a coldkey and a hotkey without registration
    //
    // # Args:
    // 	* 'origin': (<T as frame_system::Config>RuntimeOrigin):
    // 		- The signature of the calling coldkey. 
	//
    // 	* 'hotkey' ( T::AccountId ):
    // 		- Hotkey to be associated with origin coldkey.
    //   
    // # Event:
    // 	* HotkeyAssociated;
    // 		- On successfully associating a hotkey with the origin coldkey.
    //
    // # Raises:
	// 	* 'AlreadyRegistered':
    // 		- The hotkey is already associated with a coldkey.
    //

    pub fn do_associate( 
        origin: T::RuntimeOrigin,
        hotkey: T::AccountId,
		sig: T::Signature
    ) -> DispatchResult {
        // --- 1. Check that the caller has signed the transaction. (the coldkey of the pairing)
        let coldkey = ensure_signed( origin )?; 
        log::info!("do_associate( coldkey:{:?} hotkey:{:?} )", coldkey, hotkey );

		/* Hotkey bytes so we can cast to PublicKey */
		let hotkey_pubkey: MultiAddress<T::AccountId, ()> = MultiAddress::Id( hotkey.clone() );
		let binding = hotkey_pubkey.encode();
		// Skip extra 0th byte.
		let hotkey_bytes: &[u8; 32] = binding[1..].as_ref().try_into().unwrap();
		let hotkey_sr25519 = sp_core::sr25519::Public(*hotkey_bytes);

		let coldkey_str = coldkey.to_string();
		let signed_data = coldkey_str.as_bytes();

		ensure!( sig.verify( signed_data, &hotkey_sr25519 ), Error::<T>::AlreadyRegistered );
		
        // --- 2. Check the hotkey isn't already associated with a coldkey.
        ensure!( !Self::hotkey_account_exists( &hotkey ), Error::<T>::AlreadyRegistered );

        // --- 3. Creates the cold - hot pairing account if the hotkey is not already an active account.
        Self::create_account_if_non_existent( &coldkey, &hotkey );         

        // --- 4. Deposit successful event.
        log::info!("HotkeyAssociated( coldkey:{:?} hotkey:{:?} ) ", coldkey, hotkey );
        Self::deposit_event( Event::HotkeyAssociated( coldkey, hotkey ) );

        // --- 5. Ok and done.
        Ok(())
    }
    
	/// TODO( rusty ): this will take more care, edge cases if the hotkey ends up not having a coldkey but gets referenced somewhere.
    pub fn do_disassociate( 
        origin: T::RuntimeOrigin,
        hotkey: T::AccountId, 
    ) -> DispatchResult {
        // --- 1. Check that the caller has signed the transaction. (the coldkey of the pairing)
        let coldkey = ensure_signed( origin )?;
        log::info!("do_disassociate( coldkey:{:?} hotkey:{:?} )", coldkey, hotkey );

		// --- 2. Check if the origin coldkey is the owner of the hotkey
		ensure!( Self::get_owning_coldkey_for_hotkey(&hotkey) == coldkey, Error::<T>::NotHotkeyOwner );

		// --- 3. Check if hotkey is registered to a subnet
        ensure!( !Self::is_hotkey_registered_on_any_network( &hotkey ), Error::<T>::OtherAssociation );

		// --- 4. Check if hotkey is an active delegate
		ensure!( !Self::hotkey_is_delegate( &hotkey ), Error::<T>::OtherAssociation );

		// --- 5. Check if the hotkey has an active stake
		ensure!( !Self::get_stake_for_coldkey_and_hotkey( &coldkey, &hotkey ) > 0, Error::<T>::OtherAssociation );

        // --- 6. Removes the coldkey - hotkey pairing account.
        Owner::<T>::remove( &hotkey );
		Stake::<T>::remove( &coldkey, &hotkey );

        // --- 7. Deposit successful event.
        log::info!("HotkeyDisassociated( coldkey:{:?} hotkey:{:?}  ) ", coldkey, hotkey );
        Self::deposit_event( Event::HotkeyDisassociated( coldkey, hotkey ) );

        // --- 8. Ok and done.
        Ok(())
    }


    // ---- The implementation for the extrinsic do_registration.
    //
    // # Args:
    // 	* 'origin': (<T as frame_system::Config>RuntimeOrigin):
    // 		- The signature of the calling hotkey.
    //
    // 	* 'netuid' (u16):
    // 		- The u16 network identifier.
    //
    // 	* 'block_number' ( u64 ):
    // 		- Block hash used to prove work done.
    //
    // 	* 'nonce' ( u64 ):
    // 		- Positive integer nonce used in POW.
    //
    // 	* 'work' ( Vec<u8> ):
    // 		- Vector encoded bytes representing work done.
    //
    // 	* 'hotkey' ( T::AccountId ):
    // 		- Hotkey to be registered to the network.
    //
    // 	* 'coldkey' ( T::AccountId ):
    // 		- Associated coldkey account.
    //
    // # Event:
    // 	* NeuronRegistered;
    // 		- On successfully registereing a uid to a neuron slot on a subnetwork.
    //
    // # Raises:
    // 	* 'NetworkDoesNotExist':
    // 		- Attempting to registed to a non existent network.
    //
    // 	* 'TooManyRegistrationsThisBlock':
    // 		- This registration exceeds the total allowed on this network this block.
    //
    // 	* 'AlreadyRegistered':
    // 		- The hotkey is already registered on this network.
    //
    // 	* 'InvalidWorkBlock':
    // 		- The work has been performed on a stale, future, or non existent block.
    //
    // 	* 'WorkRepeated':
    // 		- This work for block has already been used.
    //
    // 	* 'InvalidDifficulty':
    // 		- The work does not match the difficutly.
    //
    // 	* 'InvalidSeal':
    // 		- The seal is incorrect.
    //
    pub fn do_registration( 
        origin: T::RuntimeOrigin,
        netuid: u16, 
        block_number: u64, 
        nonce: u64, 
        work: Vec<u8>,
        hotkey: T::AccountId, 
        coldkey: T::AccountId 
    ) -> DispatchResult {

        // --- 1. Check that the caller has signed the transaction. 
        // TODO( const ): This not be the hotkey signature or else an exterior actor can register the hotkey and potentially control it?
        let signing_origin = ensure_signed( origin )?;        
        log::info!("do_registration( origin:{:?} netuid:{:?} hotkey:{:?}, coldkey:{:?} )", signing_origin, netuid, hotkey, coldkey );

        // --- 2. Ensure the passed network is valid.
        ensure!( Self::if_subnet_exist( netuid ), Error::<T>::NetworkDoesNotExist ); 

        // --- 3. Ensure we are not exceeding the max allowed registrations per block.
        ensure!( Self::get_registrations_this_block( netuid ) < Self::get_max_registrations_per_block( netuid ), Error::<T>::TooManyRegistrationsThisBlock );

		// --- 5. Ensure we are not exceeding the max allowed registrations per interval.
		ensure!( Self::get_registrations_this_interval( netuid ) < Self::get_target_registrations_per_interval( netuid ) * 3 , Error::<T>::TooManyRegistrationsThisInterval );

        // --- 5. Ensure that the key is not already registered.
        ensure!( !Uids::<T>::contains_key( netuid, &hotkey ), Error::<T>::AlreadyRegistered );

        // --- 5. Ensure the passed block number is valid, not in the future or too old.
        // Work must have been done within 3 blocks (stops long range attacks).
        let current_block_number: u64 = Self::get_current_block_as_u64();
        ensure! (block_number <= current_block_number, Error::<T>::InvalidWorkBlock);
        ensure! (current_block_number - block_number < 3, Error::<T>::InvalidWorkBlock ); 

        // --- 6. Ensure the passed work has not already been used.
        ensure!( !UsedWork::<T>::contains_key( &work.clone() ), Error::<T>::WorkRepeated ); 

        // --- 7. Ensure the supplied work passes the difficulty.
        let difficulty: U256 = Self::get_difficulty( netuid );
        let work_hash: H256 = Self::vec_to_hash( work.clone() );
        ensure! ( Self::hash_meets_difficulty( &work_hash, difficulty ), Error::<T>::InvalidDifficulty ); // Check that the work meets difficulty.
        
        // --- 8. Check Work is the product of the nonce and the block number. Add this as used work.
        let seal: H256 = Self::create_seal_hash( block_number, nonce );
        ensure! ( seal == work_hash, Error::<T>::InvalidSeal );
        UsedWork::<T>::insert( &work.clone(), current_block_number );

        // --- 9. Ensure that the key passes the registration requirement
        ensure!( Self::passes_network_connection_requirement( netuid, &hotkey ), Error::<T>::DidNotPassConnectedNetworkRequirement );

        // --- 10. If the network account does not exist we will create it here.
        Self::create_account_if_non_existent( &coldkey, &hotkey);         

        // --- 11. Ensure that the pairing is correct.
        ensure!( Self::coldkey_owns_hotkey( &coldkey, &hotkey ), Error::<T>::NonAssociatedColdKey );

        // --- 12. Append neuron or prune it.
        let subnetwork_uid: u16;
        let current_subnetwork_n: u16 = Self::get_subnetwork_n( netuid );

        // Possibly there is no neuron slots at all.
        ensure!( Self::get_max_allowed_uids( netuid ) != 0, Error::<T>::NetworkDoesNotExist );
        
        if current_subnetwork_n < Self::get_max_allowed_uids( netuid ) {

            // --- 12.1.1 No replacement required, the uid appends the subnetwork.
            // We increment the subnetwork count here but not below.
            subnetwork_uid = current_subnetwork_n;

            // --- 12.1.2 Expand subnetwork with new account.
            Self::append_neuron( netuid, &hotkey, current_block_number );
            log::info!("add new neuron account");
        } else {
            // --- 12.1.1 Replacement required.
            // We take the neuron with the lowest pruning score here.
            subnetwork_uid = Self::get_neuron_to_prune( netuid );

            // --- 12.1.1 Replace the neuron account with the new info.
            Self::replace_neuron( netuid, subnetwork_uid, &hotkey, current_block_number );
            log::info!("prune neuron");
        }

        // --- 14. Record the registration and increment block and interval counters.
        POWRegistrationsThisInterval::<T>::mutate( netuid, |val| *val += 1 );
        RegistrationsThisInterval::<T>::mutate( netuid, |val| *val += 1 );
        RegistrationsThisBlock::<T>::mutate( netuid, |val| *val += 1 );
    
        // --- 15. Deposit successful event.
        log::info!("NeuronRegistered( netuid:{:?} uid:{:?} hotkey:{:?}  ) ", netuid, subnetwork_uid, hotkey );
        Self::deposit_event( Event::NeuronRegistered( netuid, subnetwork_uid, hotkey ) );

        // --- 16. Ok and done.
        Ok(())
    }

    // --- Checks if the hotkey passes the topk prunning requirement in all connected networks.
    //
    pub fn passes_network_connection_requirement( netuid_a: u16, hotkey: &T::AccountId ) -> bool {
        // --- 1. We are iterating over all networks to see if there is a registration connection.
        for (netuid_b, exists) in NetworksAdded::<T>::iter() {

            // --- 2. If the network exists and the registration connection requirement exists we will
            // check to see if we pass it.
            if exists && Self::network_connection_requirement_exists( netuid_a, netuid_b ){

                // --- 3. We cant be in the top percentile of an empty network.
                let subnet_n: u16 = Self::get_subnetwork_n( netuid_b );
                if subnet_n == 0 { return false; }

                // --- 4. First check to see if this hotkey is already registered on this network.
                // If we are not registered we trivially fail the requirement.
                if !Self::is_hotkey_registered_on_network( netuid_b, hotkey ) { return false; }
                let uid_b: u16 = Self::get_uid_for_net_and_hotkey( netuid_b, hotkey ).unwrap();

                // --- 5. Next, count how many keys on the connected network have a better prunning score than
                // our target network.
                let mut n_better_prunning_scores: u16 = 0;
                let our_prunning_score_b: u16 = Self::get_pruning_score_for_uid( netuid_b, uid_b );
                for other_uid in 0..subnet_n {
                    let other_runing_score_b: u16 = Self::get_pruning_score_for_uid( netuid_b, other_uid );
                    if other_uid != uid_b && other_runing_score_b > our_prunning_score_b { n_better_prunning_scores = n_better_prunning_scores + 1; }
                }

                // --- 6. Using the n_better count we check to see if the target key is in the topk percentile.
                // The percentile is stored in NetworkConnect( netuid_i, netuid_b ) as a u16 normalized value (0, 1), 1 being top 100%.
                let topk_percentile_requirement: I32F32 = I32F32::from_num( Self::get_network_connection_requirement( netuid_a, netuid_b ) ) / I32F32::from_num( u16::MAX );
                let topk_percentile_value: I32F32 = I32F32::from_num( n_better_prunning_scores ) / I32F32::from_num( Self::get_subnetwork_n( netuid_b ) );
                if topk_percentile_value > topk_percentile_requirement { return false }
            }
        }
        // --- 7. If we pass all the active registration requirments we return true allowing the registration to 
        // continue to the normal difficulty check.s
        return true;
    }

    pub fn vec_to_hash( vec_hash: Vec<u8> ) -> H256 {
        let de_ref_hash = &vec_hash; // b: &Vec<u8>
        let de_de_ref_hash: &[u8] = &de_ref_hash; // c: &[u8]
        let real_hash: H256 = H256::from_slice( de_de_ref_hash );
        return real_hash
    }

    // Determine which peer to prune from the network by finding the element with the lowest pruning score out of
    // immunity period. If all neurons are in immunity period, return node with lowest prunning score.
    // This function will always return an element to prune.
    pub fn get_neuron_to_prune(netuid: u16) -> u16 {
        let mut min_score : u16 = u16::MAX;
        let mut min_score_in_immunity_period = u16::MAX;
        let mut uid_with_min_score = 0;
        let mut uid_with_min_score_in_immunity_period: u16 =  0;
        if Self::get_subnetwork_n( netuid ) == 0 { return 0 } // If there are no neurons in this network.
        for neuron_uid_i in 0..Self::get_subnetwork_n( netuid ) {
            let pruning_score:u16 = Self::get_pruning_score_for_uid( netuid, neuron_uid_i );
            let block_at_registration: u64 = Self::get_neuron_block_at_registration( netuid, neuron_uid_i );
            let current_block :u64 = Self::get_current_block_as_u64();
            let immunity_period: u64 = Self::get_immunity_period(netuid) as u64;
            if min_score == pruning_score {
                if current_block - block_at_registration <  immunity_period { //neuron is in immunity period
                    if min_score_in_immunity_period > pruning_score {
                        min_score_in_immunity_period = pruning_score; 
                        uid_with_min_score_in_immunity_period = neuron_uid_i;
                    }
                }
                else {
                    min_score = pruning_score; 
                    uid_with_min_score = neuron_uid_i;
                }
            }
            // Find min pruning score.
            else if min_score > pruning_score { 
                if current_block - block_at_registration <  immunity_period { //neuron is in immunity period
                    if min_score_in_immunity_period > pruning_score {
                         min_score_in_immunity_period = pruning_score; 
                        uid_with_min_score_in_immunity_period = neuron_uid_i;
                    }
                }
                else {
                    min_score = pruning_score; 
                    uid_with_min_score = neuron_uid_i;
                }
            }
        }
        if min_score == u16::MAX { //all neuorns are in immunity period
            Self::set_pruning_score_for_uid( netuid, uid_with_min_score_in_immunity_period, u16::MAX );
            return uid_with_min_score_in_immunity_period;
        }
        else {
            // We replace the pruning score here with u16 max to ensure that all peers always have a 
            // pruning score. In the event that every peer has been pruned this function will prune
            // the last element in the network continually.
            Self::set_pruning_score_for_uid( netuid, uid_with_min_score, u16::MAX );
            return uid_with_min_score;
        }
    } 

    // Determine whether the given hash satisfies the given difficulty.
    // The test is done by multiplying the two together. If the product
    // overflows the bounds of U256, then the product (and thus the hash)
    // was too high.
    pub fn hash_meets_difficulty(hash: &H256, difficulty: U256) -> bool {
        let bytes: &[u8] = &hash.as_bytes();
        let num_hash: U256 = U256::from( bytes );
        let (value, overflowed) = num_hash.overflowing_mul(difficulty);

		log::trace!(
			target: LOG_TARGET,
			"Difficulty: hash: {:?}, hash_bytes: {:?}, hash_as_num: {:?}, difficulty: {:?}, value: {:?} overflowed: {:?}",
			hash,
			bytes,
			num_hash,
			difficulty,
			value,
			overflowed
		);
        !overflowed
    }

    pub fn get_block_hash_from_u64 ( block_number: u64 ) -> H256 {
        let block_number: T::BlockNumber = TryInto::<T::BlockNumber>::try_into( block_number ).ok().expect("convert u64 to block number.");
        let block_hash_at_number: <T as frame_system::Config>::Hash = system::Pallet::<T>::block_hash( block_number );
        let vec_hash: Vec<u8> = block_hash_at_number.as_ref().into_iter().cloned().collect();
        let deref_vec_hash: &[u8] = &vec_hash; // c: &[u8]
        let real_hash: H256 = H256::from_slice( deref_vec_hash );

        log::trace!(
			target: LOG_TARGET,
			"block_number: {:?}, vec_hash: {:?}, real_hash: {:?}",
			block_number,
			vec_hash,
			real_hash
		);

        return real_hash;
    }

    pub fn hash_to_vec( hash: H256 ) -> Vec<u8> {
        let hash_as_bytes: &[u8] = hash.as_bytes();
        let hash_as_vec: Vec<u8> = hash_as_bytes.iter().cloned().collect();
        return hash_as_vec
    }

    pub fn create_seal_hash( block_number_u64: u64, nonce_u64: u64 ) -> H256 {
        let nonce = U256::from( nonce_u64 );
        let block_hash_at_number: H256 = Self::get_block_hash_from_u64( block_number_u64 );
        let block_hash_bytes: &[u8] = block_hash_at_number.as_bytes();
        let full_bytes: &[u8; 40] = &[
            nonce.byte(0),  nonce.byte(1),  nonce.byte(2),  nonce.byte(3),
            nonce.byte(4),  nonce.byte(5),  nonce.byte(6),  nonce.byte(7),

            block_hash_bytes[0], block_hash_bytes[1], block_hash_bytes[2], block_hash_bytes[3],
            block_hash_bytes[4], block_hash_bytes[5], block_hash_bytes[6], block_hash_bytes[7],
            block_hash_bytes[8], block_hash_bytes[9], block_hash_bytes[10], block_hash_bytes[11],
            block_hash_bytes[12], block_hash_bytes[13], block_hash_bytes[14], block_hash_bytes[15],

            block_hash_bytes[16], block_hash_bytes[17], block_hash_bytes[18], block_hash_bytes[19],
            block_hash_bytes[20], block_hash_bytes[21], block_hash_bytes[22], block_hash_bytes[23],
            block_hash_bytes[24], block_hash_bytes[25], block_hash_bytes[26], block_hash_bytes[27],
            block_hash_bytes[28], block_hash_bytes[29], block_hash_bytes[30], block_hash_bytes[31],
        ];
        let sha256_seal_hash_vec: [u8; 32] = sha2_256( full_bytes );
        let keccak_256_seal_hash_vec: [u8; 32] = keccak_256( &sha256_seal_hash_vec );
        let seal_hash: H256 = H256::from_slice( &keccak_256_seal_hash_vec );

		log::trace!(
			"\nblock_number: {:?}, \nnonce_u64: {:?}, \nblock_hash: {:?}, \nfull_bytes: {:?}, \nsha256_seal_hash_vec: {:?},  \nkeccak_256_seal_hash_vec: {:?}, \nseal_hash: {:?}",
			block_number_u64,
			nonce_u64,
			block_hash_at_number,
			full_bytes,
			sha256_seal_hash_vec,
            keccak_256_seal_hash_vec,
			seal_hash
		);

        return seal_hash;
    }

    // Helper function for creating nonce and work.
    pub fn create_work_for_block_number( netuid:u16, block_number: u64, start_nonce: u64 ) -> (u64, Vec<u8>) {
        let difficulty: U256 = Self::get_difficulty(netuid);
        let mut nonce: u64 = start_nonce;
        let mut work: H256 = Self::create_seal_hash( block_number, nonce );
        while !Self::hash_meets_difficulty(&work, difficulty) {
            nonce = nonce + 1;
            work = Self::create_seal_hash( block_number, nonce );
        }
        let vec_work: Vec<u8> = Self::hash_to_vec( work );
        return (nonce, vec_work)
    }
}