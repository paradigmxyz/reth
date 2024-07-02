### eth-tx-summary
replay a historical tx using a web3 provider as a data source

```js
generateTxSummary(provider, targetTx, function(err, summary){
  if (err) throw err

  summary.codePath.forEach(function(step, index){
    var stepNumber = index+1
    console.log(`[${stepNumber}] ${step.pc}: ${step.opcode.name}`)
  })
  console.log(summary.results)

})
```

outputs
```
[1] 0: PUSH1
[2] 2: PUSH1
[3] 4: MSTORE
[4] 5: CALLDATASIZE
[5] 6: ISZERO
[6] 7: PUSH2
[7] 10: JUMPI
[8] 11: PUSH1
[9] 13: PUSH1
[10] 15: EXP
[11] 16: PUSH1
[12] 18: CALLDATALOAD
[13] 19: DIV
...
[322] 3776: ISZERO
[323] 3777: PUSH2
[324] 3780: JUMPI
[325] 3781: PUSH2
[326] 3784: JUMP
{ gasUsed: <BN: 249f0>,
  createdAddress: undefined,
  vm: 
   { suicides: {},
     suicideTo: undefined,
     exception: 0,
     exceptionError: 'invalid JUMP',
     logs: [],
     gas: <BN: 1ef27>,
     return: <Buffer >,
     gasUsed: <BN: 1f598> },
  bloom: { bitvector: <Buffer 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 00 ... > },
  amountSpent: <BN: b30e8870ae000> }
```