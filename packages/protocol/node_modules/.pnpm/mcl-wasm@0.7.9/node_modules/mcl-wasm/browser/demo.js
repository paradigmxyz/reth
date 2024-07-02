function getValue (name) { return document.getElementsByName(name)[0].value }
function setValue (name, val) { document.getElementsByName(name)[0].value = val }
function getText (name) { return document.getElementsByName(name)[0].innerText }
function setText (name, val) { document.getElementsByName(name)[0].innerText = val }

let prevSelectedCurve = 0
mcl.init(prevSelectedCurve).then(() => {
  setText('status', 'ok')
})

function onChangeSelectCurve () {
  const obj = document.selectCurve.curveType
  const idx = obj.selectedIndex
  const curveType = obj.options[idx].value | 0
  if (curveType === prevSelectedCurve) return
  prevSelectedCurve = curveType
  mcl.init(curveType).then(() => {
    setText('status', `curveType=${curveType} status ok`)
  })
}

// Enc(m) = [r P, m + h(e(r mpk, H(id)))]
function IDenc (id, P, mpk, m) {
  const r = new mcl.Fr()
  r.setByCSPRNG()
  const Q = mcl.hashAndMapToG2(id)
  const e = mcl.pairing(mcl.mul(mpk, r), Q)
  return [mcl.mul(P, r), mcl.add(m, mcl.hashToFr(e.serialize()))]
}

// Dec([U, v]) = v - h(e(U, sk))
function IDdec (c, sk) {
  const [U, v] = c
  const e = mcl.pairing(U, sk)
  return mcl.sub(v, mcl.hashToFr(e.serialize()))
}

function onClickIBE () {
  const P = mcl.hashAndMapToG1('1')
  // keyGen
  const msk = new mcl.Fr()
  msk.setByCSPRNG()
  setText('msk', msk.serializeToHexStr())
  // mpk = msk P
  const mpk = mcl.mul(P, msk)
  setText('mpk', mpk.serializeToHexStr())

  // user KeyGen
  const id = getText('id')
  // sk = msk H(id)
  const sk = mcl.mul(mcl.hashAndMapToG2(id), msk)
  setText('sk', sk.serializeToHexStr())

  const m = new mcl.Fr()
  const msg = getValue('msg')
  console.log('msg', msg)
  m.setStr(msg)

  // encrypt
  const c = IDenc(id, P, mpk, m)
  setText('enc', c[0].serializeToHexStr() + ' ' + c[1].serializeToHexStr())
  // decrypt
  const d = IDdec(c, sk)
  setText('dec', d.getStr())
}

function bench (label, count, func) {
  const start = performance.now()
  for (let i = 0; i < count; i++) {
    func()
  }
  const end = performance.now()
  const t = (end - start) / count
  const roundTime = (Math.round(t * 1000)) / 1000
  setText(label, roundTime)
}

function benchAll () {
  const a = new mcl.Fr()

  const msg = 'hello wasm'

  a.setByCSPRNG()
  let P = mcl.hashAndMapToG1('abc')
  let Q = mcl.hashAndMapToG2('abc')
  const P2 = mcl.hashAndMapToG1('abce')
  const Q2 = mcl.hashAndMapToG2('abce')
  const Qcoeff = new mcl.PrecomputedG2(Q)
  const e = mcl.pairing(P, Q)

  console.log('benchmark')
  const C = 100
  const C2 = 1000
  bench('T_Fr::setByCSPRNG', C, () => a.setByCSPRNG())
  bench('T_pairing', C, () => mcl.pairing(P, Q))
  bench('T_millerLoop', C, () => mcl.millerLoop(P, Q))
  bench('T_finalExp', C, () => mcl.finalExp(e))
  bench('T_precomputedMillerLoop', C, () => mcl.precomputedMillerLoop(P, Qcoeff))
  bench('T_G1::add', C2, () => { P = mcl.add(P, P2) })
  bench('T_G1::dbl', C2, () => { P = mcl.dbl(P) })
  bench('T_G1::mul', C, () => { P = mcl.mul(P, a) })
  bench('T_G2::add', C2, () => { Q = mcl.add(Q, Q2) })
  bench('T_G2::dbl', C2, () => { Q = mcl.dbl(Q) })
  bench('T_G2::mul', C, () => { Q = mcl.mul(Q, a) })
  bench('T_hashAndMapToG1', C, () => mcl.hashAndMapToG1(msg))
  bench('T_hashAndMapToG2', C, () => mcl.hashAndMapToG2(msg))

  let b = new mcl.Fr()
  b.setByCSPRNG()
  bench('T_Fr::add', C2, () => { b = mcl.add(b, a) })
  bench('T_Fr::mul', C2, () => { b = mcl.mul(b, a) })
  bench('T_Fr::sqr', C2, () => { b = mcl.sqr(b) })
  bench('T_Fr::inv', C2, () => { b = mcl.inv(b) })

  let e2 = mcl.pairing(P, Q)
  bench('T_GT::add', C2, () => { e2 = mcl.add(e2, e) })
  bench('T_GT::mul', C2, () => { e2 = mcl.mul(e2, e) })
  bench('T_GT::sqr', C2, () => { e2 = mcl.sqr(e2) })
  bench('T_GT::inv', C, () => { e2 = mcl.inv(e2) })

  Qcoeff.destroy()
}
