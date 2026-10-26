package telmoni

import (
	"context"
	_ "embed"
	"encoding/json"
	"errors"
	"fmt"
	"math"
	"sync"
)

// The guard: a run's opt-in caps on its steps and its spend, counted in this
// process with no call to Telmoni. A call's cost is counted as the server
// counts it: as reported, else its tokens at its price in the table this
// release carries (model_prices.json, LiteLLM's, whose notice it holds), else
// nothing; so a call with neither stops nothing. A price handed to the client
// comes before the table's, and the server never sees it.

// pricesFile is the price table this release carries.
//
//go:embed model_prices.json
var pricesFile []byte

// CapExceededError is a cap a run reached, the cause of the context its next
// step, model call or tool call opens with: Cap is "steps" or "spend", Limit
// the cap, and Used what the run had used of it, its steps opened or the US
// dollars it spent.
type CapExceededError struct {
	Cap   string
	Limit float64
	Used  float64
}

func (e *CapExceededError) Error() string {
	if e.Cap == "steps" {
		return fmt.Sprintf("telmoni: the run reached its step cap of %g", e.Limit)
	}
	return fmt.Sprintf("telmoni: the run reached its spend cap of $%g, having spent $%g", e.Limit, e.Used)
}

// Price is a project's own price for one of a provider's models, as the
// provider and the model are sent: each in US dollars a token, a cache price
// left nil being the input price.
type Price struct {
	Provider   string   `json:"provider"`
	Model      string   `json:"model"`
	Input      float64  `json:"input"`
	Output     float64  `json:"output"`
	CacheRead  *float64 `json:"cache_read,omitempty"`
	CacheWrite *float64 `json:"cache_write,omitempty"`
}

// rate is one name's price, each in US dollars a token.
type rate struct {
	input, output, cacheRead, cacheWrite float64
}

// tokens are a call's counts as sent, one left off counting 0.
type tokens struct {
	input, output, cacheRead, cacheWrite int
}

// cost is what t costs at r: its input less the cache's at the input price,
// each cache count at its own, and its output at the output price.
func (r rate) cost(t tokens) float64 {
	uncached := 0
	if t.cacheRead <= t.input && t.cacheWrite <= t.input-t.cacheRead {
		uncached = t.input - t.cacheRead - t.cacheWrite
	}
	return float64(uncached)*r.input +
		float64(t.cacheRead)*r.cacheRead +
		float64(t.cacheWrite)*r.cacheWrite +
		float64(t.output)*r.output
}

type name struct{ provider, model string }

// table is the price table this release carries, read once per process.
var table = sync.OnceValue(func() map[name]rate {
	var file struct {
		Prices []struct {
			Provider   string  `json:"provider"`
			Model      string  `json:"model"`
			Input      float64 `json:"input"`
			Output     float64 `json:"output"`
			CacheRead  float64 `json:"cache_read"`
			CacheWrite float64 `json:"cache_write"`
		} `json:"prices"`
	}
	rates := map[name]rate{}
	// A test holds the file to the core's, so this never fails; were it to,
	// nothing would be priced.
	if json.Unmarshal(pricesFile, &file) != nil {
		return rates
	}
	for _, row := range file.Prices {
		rates[name{row.Provider, row.Model}] = rate{row.Input, row.Output, row.CacheRead, row.CacheWrite}
	}
	return rates
})

// ownRates are a project's own prices by name, a name given twice its last,
// or the error naming the first that is no price.
func ownRates(prices []Price) (map[name]rate, error) {
	rates := make(map[name]rate, len(prices))
	for _, p := range prices {
		if p.Provider == "" || p.Model == "" {
			return nil, errors.New("telmoni: a price names its provider and its model")
		}
		cacheRead, cacheWrite := p.Input, p.Input
		if p.CacheRead != nil {
			cacheRead = *p.CacheRead
		}
		if p.CacheWrite != nil {
			cacheWrite = *p.CacheWrite
		}
		for _, amount := range []float64{p.Input, p.Output, cacheRead, cacheWrite} {
			if !dollars(amount) {
				return nil, errors.New("telmoni: a price is a finite number of US dollars a token, from 0")
			}
		}
		rates[name{p.Provider, p.Model}] = rate{p.Input, p.Output, cacheRead, cacheWrite}
	}
	return rates, nil
}

// dollars is whether amount is an amount of them: finite and from 0.
func dollars(amount float64) bool {
	return !math.IsNaN(amount) && !math.IsInf(amount, 0) && amount >= 0
}

// priced is what a call that reported no cost cost: at its own price, else at
// the table's, by its provider and model as sent, whole; false for a name
// neither holds.
func (c *Client) priced(provider, model string, t tokens) (float64, bool) {
	key := name{provider, model}
	if r, ok := c.prices[key]; ok {
		return r.cost(t), true
	}
	if r, ok := table()[key]; ok {
		return r.cost(t), true
	}
	return 0, false
}

// guard is one run's caps, 0 for none, and what it has used of them.
type guard struct {
	mu       sync.Mutex
	maxSteps int
	maxSpend float64
	steps    int
	spent    float64
	refused  error
}

// opens counts a step opening, or answers why nothing opens: the run reached
// its spend cap, for whatever opens, or its steps, for a step.
func (g *guard) opens(step bool) error {
	g.mu.Lock()
	defer g.mu.Unlock()
	if g.maxSpend > 0 && g.spent >= g.maxSpend {
		g.refused = &CapExceededError{Cap: "spend", Limit: g.maxSpend, Used: g.spent}
		return g.refused
	}
	if step {
		if g.maxSteps > 0 && g.steps >= g.maxSteps {
			g.refused = &CapExceededError{Cap: "steps", Limit: float64(g.maxSteps), Used: float64(g.steps)}
			return g.refused
		}
		g.steps++
	}
	return nil
}

// refused is ctx cancelled, cause err: what an opening past a cap returns,
// so a request made with it fails at once, as one past its deadline does.
func refused(ctx context.Context, err error) context.Context {
	if ctx == nil {
		ctx = context.Background()
	}
	ctx, cancel := context.WithCancelCause(ctx)
	cancel(err)
	return ctx
}
