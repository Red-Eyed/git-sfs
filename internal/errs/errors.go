package errs

import "errors"

var (
	ErrMissingCacheConfig = errors.New("missing cache config")
	ErrInvalidConfig      = errors.New("invalid config")
	ErrInvalidSymlink     = errors.New("invalid symlink")
	ErrMissingCachedFile  = errors.New("missing cached file")
	ErrCorruptCachedFile  = errors.New("corrupt cached file")
	ErrMissingRemoteFile  = errors.New("missing remote file")
	ErrCorruptRemoteFile  = errors.New("corrupt remote file")
)
