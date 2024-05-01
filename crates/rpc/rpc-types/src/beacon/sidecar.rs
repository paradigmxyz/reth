use alloy_primitives::{Bytes, B256};
use serde::{Deserialize, Serialize};
use serde_with::{serde_as, DisplayFromStr};

#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct BeaconBlobBundle {
    data: Vec<BlobSidecar>,
}

#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct BlobSidecar {
    #[serde_as(as = "DisplayFromStr")]
    index: u64,
    blob: String,
    kzg_commitment: Bytes,
    kzg_proof: Bytes,
    signed_block_header: SignedBlockHeader,
    kzg_commitment_inclusion_proof: Vec<B256>,
}

#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct SignedBlockHeader {
    message: BlockHeaderMessage,
    signature: Bytes,
}

#[serde_as]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
struct BlockHeaderMessage {
    #[serde_as(as = "DisplayFromStr")]
    slot: u64,
    #[serde_as(as = "DisplayFromStr")]
    proposer_index: u64,
    parent_root: B256,
    state_root: B256,
    body_root: B256,
}

//Move json data to file?
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn serde_sidecar() {
        let s = r#"{
            "index": "0",
            "blob": "0xeEf0CCEecD0aBfc3e0a3ECBb64245743e5ec2e0D25bE0C5ebEc1BDA773ac8a0DBa1fEedE8E7Aea0A7A24eeA0F4EbD3CC",
            "kzg_commitment": "0xadfebfe0d08b0b6f4e95bff79e2e029406e40781cca70c13f43104b81a03bf682ecfca570370b2836cb04251ef6656ae",
            "kzg_proof": "0xa7e8ea205bd8ab84ffa866930957ee5228b952e45c28e0ab3bb25ae7c51185f2cdf02d44af683cd513b3d5e2a0e6ee1b",
            "signed_block_header": {
                "message": {
                    "slot": "1409759",
                    "proposer_index": "110239",
                    "parent_root": "0x83c2e78d90e9d4031c0de0db5782143ac38e0e7f41ad98f8b97dff90a270e6df",
                    "state_root": "0x11122c310a39307f2d3150f9f368599dd8c5771786479314ed527002f10e6548",
                    "body_root": "0xe63dab4a3275db621ef3a3a34848d24049c61f6c0e93deaf6b179f0e8aee97b2"
                },
                "signature": "0x953e3b23dcc50ca430e7c9456a053ceba5990b1ee542c1631aa96a3cd998ba5bf0df86d027288ddac07e4100a312b44506b4c4ad8c6e1c6ce27d39d76627e1f729c3622130c97def6c76f39d29cbf1b1fe81204ed821c89389d8b61d22530455"
            },
            "kzg_commitment_inclusion_proof": [
                "0x6f375622fe38528180b8bbce850131c5c287115fc0a19693a85073289b3aa1fe",
                "0x4a97acf7425809951e2dfa23af457d1591f91a4c072fb5ae07a6c38b6ac02270",
                "0xcdfe025837f134df085d20c9f4f48ba7469b6fe66dfd3ffe68086e3331f2ff3c",
                "0xc78009fdf07fc56a11f122370658a353aaa542ed63e44c4bc15ff4cd105ab33c",
                "0x536d98837f2dd165a55d5eeae91485954472d56f246df256bf3cae19352a123c",
                "0x9efde052aa15429fae05bad4d0b1d7c64da64d03d7a1854a588c2cb8430c0d30",
                "0xd88ddfeed400a8755596b21942c1497e114c302e6118290f91e6772976041fa1",
                "0x87eb0ddba57e35f6d286673802a4af5975e22506c7cf4c64bb6be5ee11527f2c",
                "0x26846476fd5fc54a5d43385167c95144f2643f533cc85bb9d16b782f8d7db193",
                "0x506d86582d252405b840018792cad2bf1259f1ef5aa5f887e13cb2f0094f51e1",
                "0xffff0ad7e659772f9534c195c815efc4014ef1e1daed4404c06385d11192e92b",
                "0x6cf04127db05441cd833107a52be852868890e4317e6a02ab47683aa75964220",
                "0x0600000000000000000000000000000000000000000000000000000000000000",
                "0x792930bbd5baac43bcc798ee49aa8185ef76bb3b44ba62b91d86ae569e4bb535",
                "0x527b1cda425c4bf8128c2ebcd1c9d9f4d507237067a59ce5815079553b36c6f7",
                "0xdb56114e00fdd4c1f85c892bf35ac9a89289aaecb1ebd0a96cde606a748b5d71",
                "0xe01c0837cb2d1b2dcb110929f0f3922b07d6712ae1e7ba65fda1eed7d79de4af"
            ]
        }"#;

        let resp: BlobSidecar = serde_json::from_str(s).unwrap();
        let json: serde_json::Value = serde_json::from_str(s).unwrap();
        assert_eq!(json, serde_json::to_value(resp).unwrap());
    }

    #[test]
    fn serde_sidecar_bundle() {
        let s = r#"{
            "data": [
                {
                "index": "0",
                "blob": "0xeEf0CCEecD0aBfc3e0a3ECBb64245743e5ec2e0D25bE0C5ebEc1BDA773ac8a0DBa1fEedE8E7Aea0A7A24eeA0F4EbD3CC",
                "kzg_commitment": "0xadfebfe0d08b0b6f4e95bff79e2e029406e40781cca70c13f43104b81a03bf682ecfca570370b2836cb04251ef6656ae",
                "kzg_proof": "0xa7e8ea205bd8ab84ffa866930957ee5228b952e45c28e0ab3bb25ae7c51185f2cdf02d44af683cd513b3d5e2a0e6ee1b",
                "signed_block_header": {
                    "message": {
                        "slot": "1409759",
                        "proposer_index": "110239",
                        "parent_root": "0x83c2e78d90e9d4031c0de0db5782143ac38e0e7f41ad98f8b97dff90a270e6df",
                        "state_root": "0x11122c310a39307f2d3150f9f368599dd8c5771786479314ed527002f10e6548",
                        "body_root": "0xe63dab4a3275db621ef3a3a34848d24049c61f6c0e93deaf6b179f0e8aee97b2"
                    },
                    "signature": "0x953e3b23dcc50ca430e7c9456a053ceba5990b1ee542c1631aa96a3cd998ba5bf0df86d027288ddac07e4100a312b44506b4c4ad8c6e1c6ce27d39d76627e1f729c3622130c97def6c76f39d29cbf1b1fe81204ed821c89389d8b61d22530455"
                },
                "kzg_commitment_inclusion_proof": [
                    "0x6f375622fe38528180b8bbce850131c5c287115fc0a19693a85073289b3aa1fe",
                    "0x4a97acf7425809951e2dfa23af457d1591f91a4c072fb5ae07a6c38b6ac02270",
                    "0xcdfe025837f134df085d20c9f4f48ba7469b6fe66dfd3ffe68086e3331f2ff3c",
                    "0xc78009fdf07fc56a11f122370658a353aaa542ed63e44c4bc15ff4cd105ab33c",
                    "0x536d98837f2dd165a55d5eeae91485954472d56f246df256bf3cae19352a123c",
                    "0x9efde052aa15429fae05bad4d0b1d7c64da64d03d7a1854a588c2cb8430c0d30",
                    "0xd88ddfeed400a8755596b21942c1497e114c302e6118290f91e6772976041fa1",
                    "0x87eb0ddba57e35f6d286673802a4af5975e22506c7cf4c64bb6be5ee11527f2c",
                    "0x26846476fd5fc54a5d43385167c95144f2643f533cc85bb9d16b782f8d7db193",
                    "0x506d86582d252405b840018792cad2bf1259f1ef5aa5f887e13cb2f0094f51e1",
                    "0xffff0ad7e659772f9534c195c815efc4014ef1e1daed4404c06385d11192e92b",
                    "0x6cf04127db05441cd833107a52be852868890e4317e6a02ab47683aa75964220",
                    "0x0600000000000000000000000000000000000000000000000000000000000000",
                    "0x792930bbd5baac43bcc798ee49aa8185ef76bb3b44ba62b91d86ae569e4bb535",
                    "0x527b1cda425c4bf8128c2ebcd1c9d9f4d507237067a59ce5815079553b36c6f7",
                    "0xdb56114e00fdd4c1f85c892bf35ac9a89289aaecb1ebd0a96cde606a748b5d71",
                    "0xe01c0837cb2d1b2dcb110929f0f3922b07d6712ae1e7ba65fda1eed7d79de4af"
                ]
            },{
                "index": "1",
                "blob": "0xeEf0CCEecD0aBfc3e0a3ECBb64245743e5ec2e0D25bE0C5ebEc1BDA773ac8a0DBa1fEedE8E7Aea0A7A24eeA0F4EbD3CC",
                "kzg_commitment": "0xadfebfe0d08b0b6f4e95bff79e2e029406e40781cca70c13f43104b81a03bf682ecfca570370b2836cb04251ef6656ae",
                "kzg_proof": "0xa7e8ea205bd8ab84ffa866930957ee5228b952e45c28e0ab3bb25ae7c51185f2cdf02d44af683cd513b3d5e2a0e6ee1b",
                "signed_block_header": {
                    "message": {
                        "slot": "1409759",
                        "proposer_index": "110239",
                        "parent_root": "0x83c2e78d90e9d4031c0de0db5782143ac38e0e7f41ad98f8b97dff90a270e6df",
                        "state_root": "0x11122c310a39307f2d3150f9f368599dd8c5771786479314ed527002f10e6548",
                        "body_root": "0xe63dab4a3275db621ef3a3a34848d24049c61f6c0e93deaf6b179f0e8aee97b2"
                    },
                    "signature": "0x953e3b23dcc50ca430e7c9456a053ceba5990b1ee542c1631aa96a3cd998ba5bf0df86d027288ddac07e4100a312b44506b4c4ad8c6e1c6ce27d39d76627e1f729c3622130c97def6c76f39d29cbf1b1fe81204ed821c89389d8b61d22530455"
                },
                "kzg_commitment_inclusion_proof": [
                    "0x6f375622fe38528180b8bbce850131c5c287115fc0a19693a85073289b3aa1fe",
                    "0x4a97acf7425809951e2dfa23af457d1591f91a4c072fb5ae07a6c38b6ac02270",
                    "0xcdfe025837f134df085d20c9f4f48ba7469b6fe66dfd3ffe68086e3331f2ff3c",
                    "0xc78009fdf07fc56a11f122370658a353aaa542ed63e44c4bc15ff4cd105ab33c",
                    "0x536d98837f2dd165a55d5eeae91485954472d56f246df256bf3cae19352a123c",
                    "0x9efde052aa15429fae05bad4d0b1d7c64da64d03d7a1854a588c2cb8430c0d30",
                    "0xd88ddfeed400a8755596b21942c1497e114c302e6118290f91e6772976041fa1",
                    "0x87eb0ddba57e35f6d286673802a4af5975e22506c7cf4c64bb6be5ee11527f2c",
                    "0x26846476fd5fc54a5d43385167c95144f2643f533cc85bb9d16b782f8d7db193",
                    "0x506d86582d252405b840018792cad2bf1259f1ef5aa5f887e13cb2f0094f51e1",
                    "0xffff0ad7e659772f9534c195c815efc4014ef1e1daed4404c06385d11192e92b",
                    "0x6cf04127db05441cd833107a52be852868890e4317e6a02ab47683aa75964220",
                    "0x0600000000000000000000000000000000000000000000000000000000000000",
                    "0x792930bbd5baac43bcc798ee49aa8185ef76bb3b44ba62b91d86ae569e4bb535",
                    "0x527b1cda425c4bf8128c2ebcd1c9d9f4d507237067a59ce5815079553b36c6f7",
                    "0xdb56114e00fdd4c1f85c892bf35ac9a89289aaecb1ebd0a96cde606a748b5d71",
                    "0xe01c0837cb2d1b2dcb110929f0f3922b07d6712ae1e7ba65fda1eed7d79de4af"
                ]
            }
        ]}"#;

        let resp: BeaconBlobBundle = serde_json::from_str(s).unwrap();
        let json: serde_json::Value = serde_json::from_str(s).unwrap();
        assert_eq!(json, serde_json::to_value(resp.clone()).unwrap());
        assert_eq!(2, resp.data.len());
    }
}
