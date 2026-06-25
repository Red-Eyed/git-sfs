package hash

import (
	"context"
	"crypto/sha256"
	"encoding/hex"
	"fmt"
	"io"
	"os"
)

const Algorithm = "sha256"
const HexLen = 64

const chunkSize = 4 << 20

type Hash string

// File streams path through SHA-256 without loading large files into memory.
// The read loop checks ctx before each chunk so a long hash can be canceled
// promptly (within one chunk) by Ctrl-C.
func File(ctx context.Context, path string) (Hash, error) {
	return FileProgress(ctx, path, nil)
}

// FileProgress is File with a per-chunk callback. onRead, when non-nil, is
// called with the number of bytes hashed in each chunk, so callers can drive a
// byte-weighted progress bar. The hash value is identical to File's.
func FileProgress(ctx context.Context, path string, onRead func(n int)) (Hash, error) {
	f, err := os.Open(path)
	if err != nil {
		return "", err
	}
	defer f.Close()
	h := sha256.New()
	buf := make([]byte, chunkSize)
	for {
		if err := ctx.Err(); err != nil {
			return "", err
		}
		n, readErr := f.Read(buf)
		if n > 0 {
			h.Write(buf[:n])
			if onRead != nil {
				onRead(n)
			}
		}
		if readErr == io.EOF {
			break
		}
		if readErr != nil {
			return "", readErr
		}
	}
	return Hash(hex.EncodeToString(h.Sum(nil))), nil
}

// VerifyFile rejects missing files and files whose content does not match want.
func VerifyFile(ctx context.Context, path string, want Hash) error {
	got, err := File(ctx, path)
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
