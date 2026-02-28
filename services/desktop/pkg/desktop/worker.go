package desktop

import (
	"context"
	"sync"
	"time"
)

// TickWorker runs an Action on a regular interval in a background goroutine.
// It replaces the ticker+stopCh+WaitGroup pattern repeated across desktop modules
// (e.g., KillKrill flush, SkaUsWatch check-in, WaddlePerf schedule).
type TickWorker struct {
	// Interval is the period between Action invocations.
	Interval time.Duration

	// Timeout is the per-tick context deadline applied when calling Action.
	// A zero value means no per-tick timeout is applied.
	Timeout time.Duration

	// Action is the function called on each tick. Errors are forwarded to OnError.
	Action func(ctx context.Context) error

	// OnError is called whenever Action returns a non-nil error.
	// When nil, errors are silently ignored.
	OnError func(err error)

	stopCh chan struct{}
	wg     sync.WaitGroup
	mu     sync.Mutex
}

// Start spawns the background goroutine. It panics if called on an already-running worker.
func (w *TickWorker) Start() {
	w.mu.Lock()
	defer w.mu.Unlock()

	if w.stopCh != nil {
		panic("desktop: TickWorker.Start called on already-running worker")
	}

	w.stopCh = make(chan struct{})
	w.wg.Add(1)

	go w.run(w.stopCh)
}

// Stop signals the background goroutine to exit and waits for it to finish.
// It is safe to call Stop on a worker that was never started.
func (w *TickWorker) Stop() {
	w.mu.Lock()
	ch := w.stopCh
	w.stopCh = nil
	w.mu.Unlock()

	if ch == nil {
		return
	}

	close(ch)
	w.wg.Wait()
}

func (w *TickWorker) run(stopCh <-chan struct{}) {
	defer w.wg.Done()

	ticker := time.NewTicker(w.Interval)
	defer ticker.Stop()

	for {
		select {
		case <-stopCh:
			return
		case <-ticker.C:
			w.tick()
		}
	}
}

func (w *TickWorker) tick() {
	ctx := context.Background()

	if w.Timeout > 0 {
		var cancel context.CancelFunc
		ctx, cancel = context.WithTimeout(ctx, w.Timeout)
		defer cancel()
	}

	if err := w.Action(ctx); err != nil {
		if w.OnError != nil {
			w.OnError(err)
		}
	}
}
