# ExEx Op-Proposer

### Scratch Space (TODO: remove later)
- Configuration logic 
  - `SUBMISSION_INTERVAL`
  - L2OutputOracle
  - L2ToL1MessagePasser
  - Proposer pk
  - L1 RPC endpoint
  - Persistent storage endpoint/impl

- Core logic
  - Fetch next block from l2 output oracle - task
    - Get proof data for next block
      - If next block < current block
        - Get proof data from db - task dep on db logic
          - If not in db 
            - get historical proof - not implemented, need to figure out
      - else if current block < next output oracle block
        - Wait for next block
      - otherwise get the proof data at chain tip - task
        -  Store data in persistent storage - task 
  - Post transaction to L1OutputOracle
    - Tx inclusion logic


Db logic/schema - task
create types for proof data / transactions