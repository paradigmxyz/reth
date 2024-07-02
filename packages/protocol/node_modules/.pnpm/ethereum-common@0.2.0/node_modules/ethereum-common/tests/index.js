const tape = require('tape')
const params = require('../index.js')

tape('JSON validity/Param reachability', function (t) {
  
  t.test('should be able to read value from params.json', function (st) {
    st.equal(params.genesisHash.v, "0xd4e56740f876aef8c010b86a40d5f56745a118d0906a34e69aec8c0db1cb8fa3")
    st.end()
  })
  
  t.test('should be able to read value from genesisState.json', function(st) {
    st.equal(params.genesisState["3282791d6fd713f1e94f4bfd565eaa78b3a0599d"], "1337000000000000000000")
    st.end()
  })
  
  t.test('should be able to read value from bootstrapNodes.json', function (st) {
    st.equal(params.bootstrapNodes[0].port, "30303")
    st.end()
  })
  
})