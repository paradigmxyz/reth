// Generates EIP-8408 fixtures with geth's independent RLP codec.
package main

import (
	"bytes"
	"encoding/hex"
	"encoding/json"
	"os"

	"github.com/ethereum/go-ethereum/rlp"
)

const gethCommit = "24b38e763fdd7dee8b91007d5f1a94cd21c9a7f0"

type hash [32]byte
type mask [16]byte
type cell [2048]byte
type getCells struct {
	RequestID uint64
	Hashes    []hash
	Masks     []mask
	MaskIDs   []byte
}
type cells struct {
	RequestID uint64
	Hashes    []hash
	Cells     [][]cell
	Masks     []mask
	MaskIDs   []byte
}

func packet[T any](id byte, value T) string {
	encoded, err := rlp.EncodeToBytes(value)
	if err != nil {
		panic(err)
	}
	var decoded T
	if err := rlp.DecodeBytes(encoded, &decoded); err != nil {
		panic(err)
	}
	roundtrip, err := rlp.EncodeToBytes(decoded)
	if err != nil {
		panic(err)
	}
	if !bytes.Equal(encoded, roundtrip) {
		panic("geth RLP roundtrip differs")
	}
	return "0x" + hex.EncodeToString(append([]byte{id}, encoded...))
}
func main() {
	var a, b, c hash
	a[31], b[31], c[31] = 0x11, 0x22, 0x33
	var first, second mask
	first[0], first[1], first[15] = 1, 1, 0x80
	second[0], second[1] = 2, 2
	request := getCells{42, []hash{a, b, c}, []mask{first, second}, []byte{0, 1, 0}}
	// Omit a transaction, reorder the table by first use, and return mask subsets.
	var returnedFirst, returnedSecond mask
	returnedFirst[1], returnedSecond[15] = 2, 0x80
	var x, y cell
	x[0], x[2047], y[0], y[2047] = 1, 2, 3, 4
	response := cells{42, []hash{b, c}, [][]cell{{x}, {y}}, []mask{returnedFirst, returnedSecond}, []byte{0, 1}}
	empty := cells{42, []hash{}, [][]cell{}, []mask{}, []byte{}}
	vectors := struct {
		GethCommit    string `json:"geth_commit"`
		Request       string `json:"request"`
		Response      string `json:"response"`
		EmptyResponse string `json:"empty_response"`
	}{gethCommit, packet(0x14, request), packet(0x15, response), packet(0x15, empty)}
	encoder := json.NewEncoder(os.Stdout)
	encoder.SetIndent("", "  ")
	if err := encoder.Encode(vectors); err != nil {
		panic(err)
	}
}
