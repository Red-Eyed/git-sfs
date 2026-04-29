package hash

import (
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"io"
	"os"
)

const Algorithm = "sha256"
const HexLen = 64

type Hash string

// File streams path through SHA-256 without loading large files into memory.
func File(path string) (Hash, error) {
	f, err := os.Open(path)
	if err != nil {
		return "", err
	}
	defer f.Close()
	h := sha256.New()
	if _, err := io.Copy(h, f); err != nil {
		return "", err
	}
	return Hash(hex.EncodeToString(h.Sum(nil))), nil
}

// VerifyFile rejects missing files and files whose content does not match want.
func VerifyFile(path string, want Hash) error {
	got, err := File(path)
	if err != nil {
		return err
	}
	if got != want {
		return fmt.Errorf("hash mismatch for %s: got %s want %s", path, got, want)
	}
	return nil
}

func (h Hash) String() string { return string(h) }

// Prefix is the two-character fanout directory used by the file store.
func (h Hash) Prefix() string {
	s := string(h)
	if len(s) < 2 {
		return ""
	}
	return s[:2]
}

// Parse accepts only lowercase canonical SHA-256 hex strings.
func Parse(s string) (Hash, error) {
	if len(s) != HexLen {
		return "", fmt.Errorf("invalid sha256 length for %q", s)
	}
	for _, r := range s {
		if !((r >= '0' && r <= '9') || (r >= 'a' && r <= 'f')) {
			return "", fmt.Errorf("invalid sha256 hex %q", s)
		}
	}
	return Hash(s), nil
}
