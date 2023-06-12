    // use crate::{
    //     AllGenesisFormats, Chain, ChainSpec, ChainSpecBuilder, ForkCondition, ForkHash, ForkId,
    //     Genesis, Hardfork, Head, GOERLI, H256, MAINNET, SEPOLIA, U256,
    // };
    // use bytes::BytesMut;
    // use ethers_core::types as EtherType;
    // use reth_rlp::Encodable;
    // fn test_fork_ids(spec: &ChainSpec, cases: &[(Head, ForkId)]) {
    //     for (block, expected_id) in cases {
    //         let computed_id = spec.fork_id(block);
    //         assert_eq!(
    //             expected_id, &computed_id,
    //             "Expected fork ID {:?}, computed fork ID {:?} at block {}",
    //             expected_id, computed_id, block.number
    //         );
    //     }
    // }

    // // Tests that the ForkTimestamps are correctly set up.
    // #[test]
    // fn test_fork_timestamps() {
    //     let spec = ChainSpec::builder().chain(Chain::mainnet()).genesis(Genesis::default()).build();
    //     assert!(spec.fork_timestamps.shanghai.is_none());

    //     let spec = ChainSpec::builder()
    //         .chain(Chain::mainnet())
    //         .genesis(Genesis::default())
    //         .with_fork(Hardfork::Shanghai, ForkCondition::Timestamp(1337))
    //         .build();
    //     assert_eq!(spec.fork_timestamps.shanghai, Some(1337));
    //     assert!(spec.is_shanghai_activated_at_timestamp(1337));
    //     assert!(!spec.is_shanghai_activated_at_timestamp(1336));
    // }

    // // Tests that all predefined timestamps are correctly set up in the chainspecs
    // #[test]
    // fn test_predefined_chain_spec_fork_timestamps() {
    //     fn ensure_timestamp_fork_conditions(spec: &ChainSpec) {
    //         // This is a sanity test that ensures we always set all currently known fork timestamps,
    //         // this will fail if a new timestamp based fork condition has added to the hardforks but
    //         // no corresponding entry in the ForkTimestamp types, See also
    //         // [ForkTimestamps::from_hardforks]

    //         // currently there are only 1 timestamps known: shanghai
    //         let known_timestamp_based_forks = 1;
    //         let num_timestamp_based_forks =
    //             spec.hardforks.values().copied().filter(ForkCondition::is_timestamp).count();
    //         assert_eq!(num_timestamp_based_forks, known_timestamp_based_forks);

    //         // ensures all timestamp forks are set
    //         assert!(spec.fork_timestamps.shanghai.is_some());
    //     }

    //     for spec in [&*MAINNET, &*SEPOLIA] {
    //         ensure_timestamp_fork_conditions(spec);
    //     }
    // }

    // // Tests that we skip any fork blocks in block #0 (the genesis ruleset)
    // #[test]
    // fn ignores_genesis_fork_blocks() {
    //     let spec = ChainSpec::builder()
    //         .chain(Chain::mainnet())
    //         .genesis(Genesis::default())
    //         .with_fork(Hardfork::Frontier, ForkCondition::Block(0))
    //         .with_fork(Hardfork::Homestead, ForkCondition::Block(0))
    //         .with_fork(Hardfork::Tangerine, ForkCondition::Block(0))
    //         .with_fork(Hardfork::SpuriousDragon, ForkCondition::Block(0))
    //         .with_fork(Hardfork::Byzantium, ForkCondition::Block(0))
    //         .with_fork(Hardfork::Constantinople, ForkCondition::Block(0))
    //         .with_fork(Hardfork::Istanbul, ForkCondition::Block(0))
    //         .with_fork(Hardfork::MuirGlacier, ForkCondition::Block(0))
    //         .with_fork(Hardfork::Berlin, ForkCondition::Block(0))
    //         .with_fork(Hardfork::London, ForkCondition::Block(0))
    //         .with_fork(Hardfork::ArrowGlacier, ForkCondition::Block(0))
    //         .with_fork(Hardfork::GrayGlacier, ForkCondition::Block(0))
    //         .build();

    //     assert_eq!(spec.hardforks().len(), 12, "12 forks should be active.");
    //     assert_eq!(
    //         spec.fork_id(&Head { number: 1, ..Default::default() }),
    //         ForkId { hash: ForkHash::from(spec.genesis_hash()), next: 0 },
    //         "the fork ID should be the genesis hash; forks at genesis are ignored for fork filters"
    //     );
    // }

    // #[test]
    // fn ignores_duplicate_fork_blocks() {
    //     let empty_genesis = Genesis::default();
    //     let unique_spec = ChainSpec::builder()
    //         .chain(Chain::mainnet())
    //         .genesis(empty_genesis.clone())
    //         .with_fork(Hardfork::Frontier, ForkCondition::Block(0))
    //         .with_fork(Hardfork::Homestead, ForkCondition::Block(1))
    //         .build();

    //     let duplicate_spec = ChainSpec::builder()
    //         .chain(Chain::mainnet())
    //         .genesis(empty_genesis)
    //         .with_fork(Hardfork::Frontier, ForkCondition::Block(0))
    //         .with_fork(Hardfork::Homestead, ForkCondition::Block(1))
    //         .with_fork(Hardfork::Tangerine, ForkCondition::Block(1))
    //         .build();

    //     assert_eq!(
    //         unique_spec.fork_id(&Head { number: 2, ..Default::default() }),
    //         duplicate_spec.fork_id(&Head { number: 2, ..Default::default() }),
    //         "duplicate fork blocks should be deduplicated for fork filters"
    //     );
    // }

    // #[test]
    // fn mainnet_forkids() {
    //     test_fork_ids(
    //         &MAINNET,
    //         &[
    //             (
    //                 Head { number: 0, ..Default::default() },
    //                 ForkId { hash: ForkHash([0xfc, 0x64, 0xec, 0x04]), next: 1150000 },
    //             ),
    //             (
    //                 Head { number: 1150000, ..Default::default() },
    //                 ForkId { hash: ForkHash([0x97, 0xc2, 0xc3, 0x4c]), next: 1920000 },
    //             ),
    //             (
    //                 Head { number: 1920000, ..Default::default() },
    //                 ForkId { hash: ForkHash([0x91, 0xd1, 0xf9, 0x48]), next: 2463000 },
    //             ),
    //             (
    //                 Head { number: 2463000, ..Default::default() },
    //                 ForkId { hash: ForkHash([0x7a, 0x64, 0xda, 0x13]), next: 2675000 },
    //             ),
    //             (
    //                 Head { number: 2675000, ..Default::default() },
    //                 ForkId { hash: ForkHash([0x3e, 0xdd, 0x5b, 0x10]), next: 4370000 },
    //             ),
    //             (
    //                 Head { number: 4370000, ..Default::default() },
    //                 ForkId { hash: ForkHash([0xa0, 0x0b, 0xc3, 0x24]), next: 7280000 },
    //             ),
    //             (
    //                 Head { number: 7280000, ..Default::default() },
    //                 ForkId { hash: ForkHash([0x66, 0x8d, 0xb0, 0xaf]), next: 9069000 },
    //             ),
    //             (
    //                 Head { number: 9069000, ..Default::default() },
    //                 ForkId { hash: ForkHash([0x87, 0x9d, 0x6e, 0x30]), next: 9200000 },
    //             ),
    //             (
    //                 Head { number: 9200000, ..Default::default() },
    //                 ForkId { hash: ForkHash([0xe0, 0x29, 0xe9, 0x91]), next: 12244000 },
    //             ),
    //             (
    //                 Head { number: 12244000, ..Default::default() },
    //                 ForkId { hash: ForkHash([0x0e, 0xb4, 0x40, 0xf6]), next: 12965000 },
    //             ),
    //             (
    //                 Head { number: 12965000, ..Default::default() },
    //                 ForkId { hash: ForkHash([0xb7, 0x15, 0x07, 0x7d]), next: 13773000 },
    //             ),
    //             (
    //                 Head { number: 13773000, ..Default::default() },
    //                 ForkId { hash: ForkHash([0x20, 0xc3, 0x27, 0xfc]), next: 15050000 },
    //             ),
    //             (
    //                 Head { number: 15050000, ..Default::default() },
    //                 ForkId { hash: ForkHash([0xf0, 0xaf, 0xd0, 0xe3]), next: 1681338455 },
    //             ),
    //             // First Shanghai block
    //             (
    //                 Head { number: 20000000, timestamp: 1681338455, ..Default::default() },
    //                 ForkId { hash: ForkHash([0xdc, 0xe9, 0x6c, 0x2d]), next: 0 },
    //             ),
    //             // Future Shanghai block
    //             (
    //                 Head { number: 20000000, timestamp: 2000000000, ..Default::default() },
    //                 ForkId { hash: ForkHash([0xdc, 0xe9, 0x6c, 0x2d]), next: 0 },
    //             ),
    //         ],
    //     );
    // }

    // #[test]
    // fn goerli_forkids() {
    //     test_fork_ids(
    //         &GOERLI,
    //         &[
    //             (
    //                 Head { number: 0, ..Default::default() },
    //                 ForkId { hash: ForkHash([0xa3, 0xf5, 0xab, 0x08]), next: 1561651 },
    //             ),
    //             (
    //                 Head { number: 1561650, ..Default::default() },
    //                 ForkId { hash: ForkHash([0xa3, 0xf5, 0xab, 0x08]), next: 1561651 },
    //             ),
    //             (
    //                 Head { number: 1561651, ..Default::default() },
    //                 ForkId { hash: ForkHash([0xc2, 0x5e, 0xfa, 0x5c]), next: 4460644 },
    //             ),
    //             (
    //                 Head { number: 4460643, ..Default::default() },
    //                 ForkId { hash: ForkHash([0xc2, 0x5e, 0xfa, 0x5c]), next: 4460644 },
    //             ),
    //             (
    //                 Head { number: 4460644, ..Default::default() },
    //                 ForkId { hash: ForkHash([0x75, 0x7a, 0x1c, 0x47]), next: 5062605 },
    //             ),
    //             (
    //                 Head { number: 5062605, ..Default::default() },
    //                 ForkId { hash: ForkHash([0xb8, 0xc6, 0x29, 0x9d]), next: 1678832736 },
    //             ),
    //             (
    //                 Head { number: 6000000, timestamp: 1678832735, ..Default::default() },
    //                 ForkId { hash: ForkHash([0xb8, 0xc6, 0x29, 0x9d]), next: 1678832736 },
    //             ),
    //             // First Shanghai block
    //             (
    //                 Head { number: 6000001, timestamp: 1678832736, ..Default::default() },
    //                 ForkId { hash: ForkHash([0xf9, 0x84, 0x3a, 0xbf]), next: 0 },
    //             ),
    //             // Future Shanghai block
    //             (
    //                 Head { number: 6500000, timestamp: 1678832736, ..Default::default() },
    //                 ForkId { hash: ForkHash([0xf9, 0x84, 0x3a, 0xbf]), next: 0 },
    //             ),
    //         ],
    //     );
    // }

    // #[test]
    // fn sepolia_forkids() {
    //     test_fork_ids(
    //         &SEPOLIA,
    //         &[
    //             (
    //                 Head { number: 0, ..Default::default() },
    //                 ForkId { hash: ForkHash([0xfe, 0x33, 0x66, 0xe7]), next: 1735371 },
    //             ),
    //             (
    //                 Head { number: 1735370, ..Default::default() },
    //                 ForkId { hash: ForkHash([0xfe, 0x33, 0x66, 0xe7]), next: 1735371 },
    //             ),
    //             (
    //                 Head { number: 1735371, ..Default::default() },
    //                 ForkId { hash: ForkHash([0xb9, 0x6c, 0xbd, 0x13]), next: 1677557088 },
    //             ),
    //             (
    //                 Head { number: 1735372, timestamp: 1677557087, ..Default::default() },
    //                 ForkId { hash: ForkHash([0xb9, 0x6c, 0xbd, 0x13]), next: 1677557088 },
    //             ),
    //             (
    //                 Head { number: 1735372, timestamp: 1677557088, ..Default::default() },
    //                 ForkId { hash: ForkHash([0xf7, 0xf9, 0xbc, 0x08]), next: 0 },
    //             ),
    //         ],
    //     );
    // }

    // /// Checks that time-based forks work
    // ///
    // /// This is based off of the test vectors here: https://github.com/ethereum/go-ethereum/blob/5c8cc10d1e05c23ff1108022f4150749e73c0ca1/core/forkid/forkid_test.go#L155-L188
    // #[test]
    // fn timestamped_forks() {
    //     let mainnet_with_shanghai = ChainSpecBuilder::mainnet()
    //         .with_fork(Hardfork::Shanghai, ForkCondition::Timestamp(1668000000))
    //         .build();
    //     test_fork_ids(
    //         &mainnet_with_shanghai,
    //         &[
    //             (
    //                 Head { number: 0, timestamp: 0, ..Default::default() },
    //                 ForkId { hash: ForkHash([0xfc, 0x64, 0xec, 0x04]), next: 1150000 },
    //             ), // Unsynced
    //             (
    //                 Head { number: 1149999, timestamp: 0, ..Default::default() },
    //                 ForkId { hash: ForkHash([0xfc, 0x64, 0xec, 0x04]), next: 1150000 },
    //             ), // Last Frontier block
    //             (
    //                 Head { number: 1150000, timestamp: 0, ..Default::default() },
    //                 ForkId { hash: ForkHash([0x97, 0xc2, 0xc3, 0x4c]), next: 1920000 },
    //             ), // First Homestead block
    //             (
    //                 Head { number: 1919999, timestamp: 0, ..Default::default() },
    //                 ForkId { hash: ForkHash([0x97, 0xc2, 0xc3, 0x4c]), next: 1920000 },
    //             ), // Last Homestead block
    //             (
    //                 Head { number: 1920000, timestamp: 0, ..Default::default() },
    //                 ForkId { hash: ForkHash([0x91, 0xd1, 0xf9, 0x48]), next: 2463000 },
    //             ), // First DAO block
    //             (
    //                 Head { number: 2462999, timestamp: 0, ..Default::default() },
    //                 ForkId { hash: ForkHash([0x91, 0xd1, 0xf9, 0x48]), next: 2463000 },
    //             ), // Last DAO block
    //             (
    //                 Head { number: 2463000, timestamp: 0, ..Default::default() },
    //                 ForkId { hash: ForkHash([0x7a, 0x64, 0xda, 0x13]), next: 2675000 },
    //             ), // First Tangerine block
    //             (
    //                 Head { number: 2674999, timestamp: 0, ..Default::default() },
    //                 ForkId { hash: ForkHash([0x7a, 0x64, 0xda, 0x13]), next: 2675000 },
    //             ), // Last Tangerine block
    //             (
    //                 Head { number: 2675000, timestamp: 0, ..Default::default() },
    //                 ForkId { hash: ForkHash([0x3e, 0xdd, 0x5b, 0x10]), next: 4370000 },
    //             ), // First Spurious block
    //             (
    //                 Head { number: 4369999, timestamp: 0, ..Default::default() },
    //                 ForkId { hash: ForkHash([0x3e, 0xdd, 0x5b, 0x10]), next: 4370000 },
    //             ), // Last Spurious block
    //             (
    //                 Head { number: 4370000, timestamp: 0, ..Default::default() },
    //                 ForkId { hash: ForkHash([0xa0, 0x0b, 0xc3, 0x24]), next: 7280000 },
    //             ), // First Byzantium block
    //             (
    //                 Head { number: 7279999, timestamp: 0, ..Default::default() },
    //                 ForkId { hash: ForkHash([0xa0, 0x0b, 0xc3, 0x24]), next: 7280000 },
    //             ), // Last Byzantium block
    //             (
    //                 Head { number: 7280000, timestamp: 0, ..Default::default() },
    //                 ForkId { hash: ForkHash([0x66, 0x8d, 0xb0, 0xaf]), next: 9069000 },
    //             ), // First and last Constantinople, first Petersburg block
    //             (
    //                 Head { number: 9068999, timestamp: 0, ..Default::default() },
    //                 ForkId { hash: ForkHash([0x66, 0x8d, 0xb0, 0xaf]), next: 9069000 },
    //             ), // Last Petersburg block
    //             (
    //                 Head { number: 9069000, timestamp: 0, ..Default::default() },
    //                 ForkId { hash: ForkHash([0x87, 0x9d, 0x6e, 0x30]), next: 9200000 },
    //             ), // First Istanbul and first Muir Glacier block
    //             (
    //                 Head { number: 9199999, timestamp: 0, ..Default::default() },
    //                 ForkId { hash: ForkHash([0x87, 0x9d, 0x6e, 0x30]), next: 9200000 },
    //             ), // Last Istanbul and first Muir Glacier block
    //             (
    //                 Head { number: 9200000, timestamp: 0, ..Default::default() },
    //                 ForkId { hash: ForkHash([0xe0, 0x29, 0xe9, 0x91]), next: 12244000 },
    //             ), // First Muir Glacier block
    //             (
    //                 Head { number: 12243999, timestamp: 0, ..Default::default() },
    //                 ForkId { hash: ForkHash([0xe0, 0x29, 0xe9, 0x91]), next: 12244000 },
    //             ), // Last Muir Glacier block
    //             (
    //                 Head { number: 12244000, timestamp: 0, ..Default::default() },
    //                 ForkId { hash: ForkHash([0x0e, 0xb4, 0x40, 0xf6]), next: 12965000 },
    //             ), // First Berlin block
    //             (
    //                 Head { number: 12964999, timestamp: 0, ..Default::default() },
    //                 ForkId { hash: ForkHash([0x0e, 0xb4, 0x40, 0xf6]), next: 12965000 },
    //             ), // Last Berlin block
    //             (
    //                 Head { number: 12965000, timestamp: 0, ..Default::default() },
    //                 ForkId { hash: ForkHash([0xb7, 0x15, 0x07, 0x7d]), next: 13773000 },
    //             ), // First London block
    //             (
    //                 Head { number: 13772999, timestamp: 0, ..Default::default() },
    //                 ForkId { hash: ForkHash([0xb7, 0x15, 0x07, 0x7d]), next: 13773000 },
    //             ), // Last London block
    //             (
    //                 Head { number: 13773000, timestamp: 0, ..Default::default() },
    //                 ForkId { hash: ForkHash([0x20, 0xc3, 0x27, 0xfc]), next: 15050000 },
    //             ), // First Arrow Glacier block
    //             (
    //                 Head { number: 15049999, timestamp: 0, ..Default::default() },
    //                 ForkId { hash: ForkHash([0x20, 0xc3, 0x27, 0xfc]), next: 15050000 },
    //             ), // Last Arrow Glacier block
    //             (
    //                 Head { number: 15050000, timestamp: 0, ..Default::default() },
    //                 ForkId { hash: ForkHash([0xf0, 0xaf, 0xd0, 0xe3]), next: 1668000000 },
    //             ), // First Gray Glacier block
    //             (
    //                 Head { number: 19999999, timestamp: 1667999999, ..Default::default() },
    //                 ForkId { hash: ForkHash([0xf0, 0xaf, 0xd0, 0xe3]), next: 1668000000 },
    //             ), // Last Gray Glacier block
    //             (
    //                 Head { number: 20000000, timestamp: 1668000000, ..Default::default() },
    //                 ForkId { hash: ForkHash([0x71, 0x14, 0x76, 0x44]), next: 0 },
    //             ), // First Shanghai block
    //             (
    //                 Head { number: 20000000, timestamp: 2668000000, ..Default::default() },
    //                 ForkId { hash: ForkHash([0x71, 0x14, 0x76, 0x44]), next: 0 },
    //             ), // Future Shanghai block
    //         ],
    //     );
    // }

    // /// Checks that the fork is not active at a terminal ttd block.
    // #[test]
    // fn check_terminal_ttd() {
    //     let chainspec = ChainSpecBuilder::mainnet().build();

    //     // Check that Paris is not active on terminal PoW block #15537393.
    //     let terminal_block_ttd = U256::from(58750003716598352816469_u128);
    //     let terminal_block_difficulty = U256::from(11055787484078698_u128);
    //     assert!(!chainspec
    //         .fork(Hardfork::Paris)
    //         .active_at_ttd(terminal_block_ttd, terminal_block_difficulty));

    //     // Check that Paris is active on first PoS block #15537394.
    //     let first_pos_block_ttd = U256::from(58750003716598352816469_u128);
    //     let first_pos_difficulty = U256::ZERO;
    //     assert!(chainspec
    //         .fork(Hardfork::Paris)
    //         .active_at_ttd(first_pos_block_ttd, first_pos_difficulty));
    // }

    // #[test]
    // fn geth_genesis_with_shanghai() {
    //     let geth_genesis = r#"
    //     {
    //       "config": {
    //         "chainId": 1337,
    //         "homesteadBlock": 0,
    //         "eip150Block": 0,
    //         "eip150Hash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    //         "eip155Block": 0,
    //         "eip158Block": 0,
    //         "byzantiumBlock": 0,
    //         "constantinopleBlock": 0,
    //         "petersburgBlock": 0,
    //         "istanbulBlock": 0,
    //         "muirGlacierBlock": 0,
    //         "berlinBlock": 0,
    //         "londonBlock": 0,
    //         "arrowGlacierBlock": 0,
    //         "grayGlacierBlock": 0,
    //         "shanghaiTime": 0,
    //         "terminalTotalDifficulty": 0,
    //         "terminalTotalDifficultyPassed": true,
    //         "ethash": {}
    //       },
    //       "nonce": "0x0",
    //       "timestamp": "0x0",
    //       "extraData": "0x",
    //       "gasLimit": "0x4c4b40",
    //       "difficulty": "0x1",
    //       "mixHash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    //       "coinbase": "0x0000000000000000000000000000000000000000",
    //       "alloc": {
    //         "658bdf435d810c91414ec09147daa6db62406379": {
    //           "balance": "0x487a9a304539440000"
    //         },
    //         "aa00000000000000000000000000000000000000": {
    //           "code": "0x6042",
    //           "storage": {
    //             "0x0000000000000000000000000000000000000000000000000000000000000000": "0x0000000000000000000000000000000000000000000000000000000000000000",
    //             "0x0100000000000000000000000000000000000000000000000000000000000000": "0x0100000000000000000000000000000000000000000000000000000000000000",
    //             "0x0200000000000000000000000000000000000000000000000000000000000000": "0x0200000000000000000000000000000000000000000000000000000000000000",
    //             "0x0300000000000000000000000000000000000000000000000000000000000000": "0x0000000000000000000000000000000000000000000000000000000000000303"
    //           },
    //           "balance": "0x1",
    //           "nonce": "0x1"
    //         },
    //         "bb00000000000000000000000000000000000000": {
    //           "code": "0x600154600354",
    //           "storage": {
    //             "0x0000000000000000000000000000000000000000000000000000000000000000": "0x0000000000000000000000000000000000000000000000000000000000000000",
    //             "0x0100000000000000000000000000000000000000000000000000000000000000": "0x0100000000000000000000000000000000000000000000000000000000000000",
    //             "0x0200000000000000000000000000000000000000000000000000000000000000": "0x0200000000000000000000000000000000000000000000000000000000000000",
    //             "0x0300000000000000000000000000000000000000000000000000000000000000": "0x0000000000000000000000000000000000000000000000000000000000000303"
    //           },
    //           "balance": "0x2",
    //           "nonce": "0x1"
    //         }
    //       },
    //       "number": "0x0",
    //       "gasUsed": "0x0",
    //       "parentHash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    //       "baseFeePerGas": "0x3b9aca00"
    //     }
    //     "#;

    //     let genesis: ethers_core::utils::Genesis = serde_json::from_str(geth_genesis).unwrap();
    //     let chainspec = ChainSpec::from(genesis);

    //     // assert a bunch of hardforks that should be set
    //     assert_eq!(
    //         chainspec.hardforks.get(&Hardfork::Homestead).unwrap(),
    //         &ForkCondition::Block(0)
    //     );
    //     assert_eq!(
    //         chainspec.hardforks.get(&Hardfork::Tangerine).unwrap(),
    //         &ForkCondition::Block(0)
    //     );
    //     assert_eq!(
    //         chainspec.hardforks.get(&Hardfork::SpuriousDragon).unwrap(),
    //         &ForkCondition::Block(0)
    //     );
    //     assert_eq!(
    //         chainspec.hardforks.get(&Hardfork::Byzantium).unwrap(),
    //         &ForkCondition::Block(0)
    //     );
    //     assert_eq!(
    //         chainspec.hardforks.get(&Hardfork::Constantinople).unwrap(),
    //         &ForkCondition::Block(0)
    //     );
    //     assert_eq!(
    //         chainspec.hardforks.get(&Hardfork::Petersburg).unwrap(),
    //         &ForkCondition::Block(0)
    //     );
    //     assert_eq!(chainspec.hardforks.get(&Hardfork::Istanbul).unwrap(), &ForkCondition::Block(0));
    //     assert_eq!(
    //         chainspec.hardforks.get(&Hardfork::MuirGlacier).unwrap(),
    //         &ForkCondition::Block(0)
    //     );
    //     assert_eq!(chainspec.hardforks.get(&Hardfork::Berlin).unwrap(), &ForkCondition::Block(0));
    //     assert_eq!(chainspec.hardforks.get(&Hardfork::London).unwrap(), &ForkCondition::Block(0));
    //     assert_eq!(
    //         chainspec.hardforks.get(&Hardfork::ArrowGlacier).unwrap(),
    //         &ForkCondition::Block(0)
    //     );
    //     assert_eq!(
    //         chainspec.hardforks.get(&Hardfork::GrayGlacier).unwrap(),
    //         &ForkCondition::Block(0)
    //     );

    //     // including time based hardforks
    //     assert_eq!(
    //         chainspec.hardforks.get(&Hardfork::Shanghai).unwrap(),
    //         &ForkCondition::Timestamp(0)
    //     );

    //     // alloc key -> expected rlp mapping
    //     let key_rlp = vec![
    //         (hex_literal::hex!("658bdf435d810c91414ec09147daa6db62406379"), "f84d8089487a9a304539440000a056e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421a0c5d2460186f7233c927e7db2dcc703c0e500b653ca82273b7bfad8045d85a470"),
    //         (hex_literal::hex!("aa00000000000000000000000000000000000000"), "f8440101a08afc95b7d18a226944b9c2070b6bda1c3a36afcc3730429d47579c94b9fe5850a0ce92c756baff35fa740c3557c1a971fd24d2d35b7c8e067880d50cd86bb0bc99"),
    //         (hex_literal::hex!("bb00000000000000000000000000000000000000"), "f8440102a08afc95b7d18a226944b9c2070b6bda1c3a36afcc3730429d47579c94b9fe5850a0e25a53cbb501cec2976b393719c63d832423dd70a458731a0b64e4847bbca7d2"),
    //     ];

    //     for (key, expected_rlp) in key_rlp {
    //         let account = chainspec.genesis.alloc.get(&key.into()).expect("account should exist");
    //         let mut account_rlp = BytesMut::new();
    //         account.encode(&mut account_rlp);
    //         assert_eq!(hex::encode(account_rlp), expected_rlp)
    //     }

    //     assert_eq!(chainspec.genesis_hash, None);
    //     let expected_state_root: H256 =
    //         hex_literal::hex!("078dc6061b1d8eaa8493384b59c9c65ceb917201221d08b80c4de6770b6ec7e7")
    //             .into();
    //     assert_eq!(chainspec.genesis_header().state_root, expected_state_root);

    //     let expected_withdrawals_hash: H256 =
    //         hex_literal::hex!("56e81f171bcc55a6ff8345e692c0f86e5b48e01b996cadc001622fb5e363b421")
    //             .into();
    //     assert_eq!(chainspec.genesis_header().withdrawals_root, Some(expected_withdrawals_hash));

    //     let expected_hash: H256 =
    //         hex_literal::hex!("1fc027d65f820d3eef441ebeec139ebe09e471cf98516dce7b5643ccb27f418c")
    //             .into();
    //     let hash = chainspec.genesis_hash();
    //     assert_eq!(hash, expected_hash);
    // }

    // #[test]
    // fn hive_geth_json() {
    //     let hive_json = r#"
    //     {
    //         "nonce": "0x0000000000000042",
    //         "difficulty": "0x2123456",
    //         "mixHash": "0x123456789abcdef123456789abcdef123456789abcdef123456789abcdef1234",
    //         "coinbase": "0xaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa",
    //         "timestamp": "0x123456",
    //         "parentHash": "0x0000000000000000000000000000000000000000000000000000000000000000",
    //         "extraData": "0xfafbfcfd",
    //         "gasLimit": "0x2fefd8",
    //         "alloc": {
    //             "dbdbdb2cbd23b783741e8d7fcf51e459b497e4a6": {
    //                 "balance": "0xffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff"
    //             },
    //             "e6716f9544a56c530d868e4bfbacb172315bdead": {
    //                 "balance": "0x11",
    //                 "code": "0x12"
    //             },
    //             "b9c015918bdaba24b4ff057a92a3873d6eb201be": {
    //                 "balance": "0x21",
    //                 "storage": {
    //                     "0x0000000000000000000000000000000000000000000000000000000000000001": "0x22"
    //                 }
    //             },
    //             "1a26338f0d905e295fccb71fa9ea849ffa12aaf4": {
    //                 "balance": "0x31",
    //                 "nonce": "0x32"
    //             },
    //             "0000000000000000000000000000000000000001": {
    //                 "balance": "0x41"
    //             },
    //             "0000000000000000000000000000000000000002": {
    //                 "balance": "0x51"
    //             },
    //             "0000000000000000000000000000000000000003": {
    //                 "balance": "0x61"
    //             },
    //             "0000000000000000000000000000000000000004": {
    //                 "balance": "0x71"
    //             }
    //         },
    //         "config": {
    //             "ethash": {},
    //             "chainId": 10,
    //             "homesteadBlock": 0,
    //             "eip150Block": 0,
    //             "eip155Block": 0,
    //             "eip158Block": 0,
    //             "byzantiumBlock": 0,
    //             "constantinopleBlock": 0,
    //             "petersburgBlock": 0,
    //             "istanbulBlock": 0
    //         }
    //     }
    //     "#;
    //     let genesis = serde_json::from_str::<AllGenesisFormats>(hive_json).unwrap();
    //     let chainspec: ChainSpec = genesis.into();
    //     assert_eq!(chainspec.genesis_hash, None);
    //     assert_eq!(Chain::Named(EtherType::Chain::Optimism), chainspec.chain);
    //     let expected_state_root: H256 =
    //         hex_literal::hex!("9a6049ac535e3dc7436c189eaa81c73f35abd7f282ab67c32944ff0301d63360")
    //             .into();
    //     assert_eq!(chainspec.genesis_header().state_root, expected_state_root);
    //     let hard_forks = vec![
    //         Hardfork::Byzantium,
    //         Hardfork::Homestead,
    //         Hardfork::Istanbul,
    //         Hardfork::Petersburg,
    //         Hardfork::Constantinople,
    //     ];
    //     for ref fork in hard_forks {
    //         assert_eq!(chainspec.hardforks.get(fork).unwrap(), &ForkCondition::Block(0));
    //     }

    //     let expected_hash: H256 =
    //         hex_literal::hex!("5ae31c6522bd5856129f66be3d582b842e4e9faaa87f21cce547128339a9db3c")
    //             .into();
    //     let hash = chainspec.genesis_header().hash_slow();
    //     assert_eq!(hash, expected_hash);
    // }
