// see full article here https://wanderer.github.io/ethereum/2014/06/14/creating-and-verifying-transaction-with-node/

var Transaction = require('../index.js')

// create a blank transaction
var tx = new Transaction(null, 1) // mainnet Tx EIP155

// So now we have created a blank transaction but Its not quiet valid yet. We
// need to add some things to it. Lets start:
// notice we don't set the `to` field because we are creating a new contract.
tx.nonce = 0
tx.gasPrice = 100
tx.gasLimit = 1000
tx.value = 0
tx.data = '0x7f4e616d65526567000000000000000000000000000000000000000000000000003057307f4e616d6552656700000000000000000000000000000000000000000000000000573360455760415160566000396000f20036602259604556330e0f600f5933ff33560f601e5960003356576000335700604158600035560f602b590033560f60365960003356573360003557600035335700'

var privateKey = new Buffer('e331b6d69882b4cb4ea581d88e0b604039a3de5967688d3dcffdd2270c0fd109', 'hex')
tx.sign(privateKey)
// We have a signed transaction, Now for it to be fully fundable the account that we signed
// it with needs to have a certain amount of wei in to. To see how much this
// account needs we can use the getUpfrontCost() method.
var feeCost = tx.getUpfrontCost()
tx.gas = feeCost
console.log('Total Amount of wei needed:' + feeCost.toString())

// if your wondering how that is caculated it is
// bytes(data length) * 5
// + 500 Default transaction fee
// + gasAmount * gasPrice

// lets serialize the transaction

console.log('---Serialized TX----')
console.log(tx.serialize().toString('hex'))
console.log('--------------------')

// Now that we have the serialized transaction we can get AlethZero to except by
// selecting debug>inject transaction and pasting the transaction serialization and
// it should show up in pending transaction.

// Parsing & Validating transactions
// If you have a transaction that you want to verify you can parse it. If you got
// it directly from the network it will be rlp encoded. You can decode you the rlp
// module. After that you should have something like
var rawTx = [
  '0x00',
  '0x09184e72a000',
  '0x2710',
  '0x0000000000000000000000000000000000000000',
  '0x00',
  '0x7f7465737432000000000000000000000000000000000000000000000000000000600057',
  '0x1c',
  '0x5e1d3a76fbf824220eafc8c79ad578ad2b67d01b0c2425eb1f1347e8f50882ab',
  '0x5bd428537f05f9830e93792f90ea6a3e2d1ee84952dd96edbae9f658f831ab13'
]

var tx2 = new Transaction(rawTx)

// Note rlp.decode will actully produce an array of buffers `new Transaction` will
// take either an array of buffers or an array of hex strings.
// So assuming that you were able to parse the tranaction, we will now get the sender's
// address

console.log('Senders Address: ' + tx2.getSenderAddress().toString('hex'))

// Cool now we know who sent the tx! Lets verfy the signature to make sure it was not
// some poser.

if (tx2.verifySignature()) {
  console.log('Signature Checks out!')
}

// And hopefully its verified. For the transaction to be totally valid we would
// also need to check the account of the sender and see if they have at least
// `TotalFee`.
