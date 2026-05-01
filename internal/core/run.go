package core

import (
	"context"
	"sync"
)

// runIndexed fans out work(0)..work(count-1) across workers goroutines.
// On first error it cancels remaining work and calls fail for that index.
func runIndexed(ctx context.Context, count, workers int, work func(int) error, fail func(int, error)) {
	if count == 0 {
		return
	}
	workers = jobsFromSettings(workers, count)
	ctx, cancel := context.WithCancel(ctx)
	defer cancel()
	jobs := make(chan int)
	var once sync.Once
	var wg sync.WaitGroup
	for i := 0; i < workers; i++ {
		wg.Add(1)
		go func() {
			defer wg.Done()
			for idx := range jobs {
				if ctx.Err() != nil {
					return
				}
				if err := work(idx); err != nil {
					fail(idx, err)
					once.Do(cancel)
					return
				}
			}
		}()
	}
	for i := 0; i < count; i++ {
		select {
		case <-ctx.Done():
			close(jobs)
			wg.Wait()
			return
		case jobs <- i:
		}
	}
	close(jobs)
	wg.Wait()
}
