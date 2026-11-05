package telmoni_test

import (
	"bytes"
	"context"
	"crypto/sha256"
	"encoding/base64"
	"encoding/hex"
	"encoding/json"
	"errors"
	"maps"
	"os"
	"reflect"
	"strings"
	"testing"
	"time"

	telmoni "github.com/telmoni/telmoni/sdk/go"
)

type canaryCase struct {
	Name   string `json:"name"`
	Text   string `json:"text"`
	Expect map[string]struct {
		Text     string         `json:"text"`
		Scrubbed map[string]int `json:"scrubbed"`
	} `json:"expect"`
}

type canaryFixture struct {
	Planted map[string]string `json:"planted"`
	Modes   map[string]struct {
		ScrubSecrets bool `json:"scrub_secrets"`
		ScrubPII     bool `json:"scrub_pii"`
	} `json:"modes"`
	Cases []canaryCase `json:"cases"`
}

func loadCanary(t *testing.T) canaryFixture {
	t.Helper()
	var fixture canaryFixture
	if err := json.Unmarshal(loadFixture(t, "canary.json"), &fixture); err != nil {
		t.Fatalf("decoding canary.json: %v", err)
	}
	return fixture
}

func (f canaryFixture) named(t *testing.T, name string) canaryCase {
	t.Helper()
	for _, c := range f.Cases {
		if c.Name == name {
			return c
		}
	}
	t.Fatalf("canary.json has no case %q", name)
	return canaryCase{}
}

// expectScrubbed fails unless span carries telmoni.scrubbed holding want, as
// JSON values, or none where want is empty.
func expectScrubbed(t *testing.T, what string, span map[string]any, want map[string]int) {
	t.Helper()
	got, sent := attributesOf(t, span)["telmoni.scrubbed"]
	if !sent {
		if len(want) > 0 {
			t.Errorf("%s carries no telmoni.scrubbed, want %v", what, want)
		}
		return
	}
	text, _ := got.(string)
	var counts map[string]int
	if err := json.Unmarshal([]byte(text), &counts); err != nil || len(counts) == 0 || !maps.Equal(counts, want) {
		t.Errorf("%s carries telmoni.scrubbed %v, want %v", what, got, want)
	}
}

// unsetScrubbing leaves both switches' variables unset while the test runs.
func unsetScrubbing(t *testing.T) {
	t.Helper()
	for _, name := range []string{"TELMONI_SCRUB_SECRETS", "TELMONI_SCRUB_PII"} {
		// Setenv first, so the variable is put back as it was when the test ends.
		t.Setenv(name, "")
		os.Unsetenv(name)
	}
}

func TestTheScrubbersTurnEveryCanaryCaseIntoWhatItExpects(t *testing.T) {
	fixture := loadCanary(t)
	if len(fixture.Cases) == 0 || len(fixture.Modes) != 3 {
		t.Fatalf("canary.json holds %d cases in %d modes", len(fixture.Cases), len(fixture.Modes))
	}
	for _, c := range fixture.Cases {
		for mode, switches := range fixture.Modes {
			want, ok := c.Expect[mode]
			if !ok {
				t.Errorf("%s: no expectation for %s", c.Name, mode)
				continue
			}
			text, counts := telmoni.Scrub(c.Text, switches.ScrubSecrets, switches.ScrubPII)
			if text != want.Text {
				t.Errorf("%s, %s: scrubbed to\n%q\nwant\n%q", c.Name, mode, text, want.Text)
			}
			if !maps.Equal(counts, want.Scrubbed) {
				t.Errorf("%s, %s: counted %v, want %v", c.Name, mode, counts, want.Scrubbed)
			}
		}
	}
}

// A keyword's value is counted in characters, never bytes: ä is one.
func TestAKeywordsValueIsCountedInCharacters(t *testing.T) {
	for given, want := range map[string]string{
		"password=äöüäöü12":      "password=[secret: password]",
		"password=äöüä12":        "password=äöüä12",
		"token=äöüäöüäöüäöüäö1a": "token=[secret: token]",
		"token=äöüäöüäöüäöüä1a":  "token=äöüäöüäöüäöüä1a",
	} {
		if got, _ := telmoni.Scrub(given, true, false); got != want {
			t.Errorf("%q scrubbed to %q, want %q", given, got, want)
		}
	}
}

// Each hostile text, a unit repeated past 256 KiB, is scrubbed in either
// default mode within 2 seconds, where reading a run again from each start in
// it takes minutes.
func TestTheScrubbersReadHostileTextsInLinearTime(t *testing.T) {
	for _, hostile := range []struct{ unit, tail string }{
		{"eyJ-", ""}, {"eyJ", ""}, {"token=", ""}, {"password=", ""}, {"atm_", ""}, {"xsk-ant-", ""},
		{"xoxb-", "_"}, {"a", ""}, {"a@", ""}, {"a@b.", ""}, {"x://", ""}, {"a:b", " "}, {"a:b ", ""},
		{"1", ""}, {"1 ", ""}, {"1.", ""}, {"a:", ""}, {"/home/", ""}, {"AB12", ""},
		{"-----BEGIN PRIVATE KEY-----", ""}, {"+1 ", ""},
		// Beyond the shared corpus: an IBAN's groups, keys cut short at a quote,
		// keys whose every quote is escaped, and one alternative's keys while the
		// others find none.
		{"AB12 ", ""}, {`-----BEGIN PRIVATE KEY-----"`, ""}, {`-----BEGIN PRIVATE KEY-----\"-----END PRIVATE KEY-----`, ""},
		{"whsec_AAAAAAAAAAAAAAAA ", ""},
	} {
		text := strings.Repeat(hostile.unit, 256<<10/len(hostile.unit)+1) + hostile.tail
		for mode, pii := range map[string]bool{"sdk_default": false, "plugin_default": true} {
			start := time.Now()
			telmoni.Scrub(text, true, pii)
			if took := time.Since(start); took > 2*time.Second {
				t.Errorf("%q repeated, then %q, took %v in %s", hostile.unit, hostile.tail, took, mode)
			}
		}
	}
}

// Each mode's switches reach what leaves: a message's JSON text, a tool's
// result, a failure's status and event, each span changed counting what went.
func TestContentLeavesScrubbedWithWhatWentCountedOnItsSpan(t *testing.T) {
	unsetScrubbing(t)
	fixture := loadCanary(t)
	prose := fixture.named(t, "every planted value in one prose text")
	message := fixture.named(t, "a message's JSON text, its content escaped")
	key := fixture.Planted["cloud_key"]
	for mode, cfg := range map[string]telmoni.Config{
		"sdk_default":    {},
		"plugin_default": {ScrubPII: new(true)},
		"off":            {ScrubSecrets: new(false)},
	} {
		cfg.APIKey, cfg.CaptureContent = telmoni.NewSecret("k"), true
		h := newHarness(t, cfg, newFakeTransport(t, contentOn(), contentOn()))
		aRun(t, h)
		ctx, run := h.client.Run(context.Background(), "support-bot", telmoni.RunOptions{})
		_, call := run.ModelCall(ctx, "gpt-4o", telmoni.ModelCallOptions{InputMessages: message.Text})
		call.End()
		_, tool := run.ToolCall(ctx, "read_file", telmoni.ToolCallOptions{})
		tool.Returned(prose.Text)
		tool.End()
		run.Fail(errors.New("denied for " + key))
		run.End()
		h.flush()

		spans := byName(h.transport.spans(t))
		for name, field := range map[string]struct {
			key  string
			sent canaryCase
		}{
			"chat gpt-4o":            {"gen_ai.input.messages", message},
			"execute_tool read_file": {"gen_ai.tool.call.result", prose},
		} {
			want := field.sent.Expect[mode]
			if got := attributesOf(t, spans[name])[field.key]; got != want.Text {
				t.Errorf("%s: %s sent %q, want %q", mode, field.key, got, want.Text)
			}
			expectScrubbed(t, mode+": "+name, spans[name], want.Scrubbed)
		}

		failure, counted := "denied for "+key, map[string]int{}
		if mode != "off" {
			failure, counted = "denied for [secret: aws_key]", map[string]int{"secret:aws_key": 2}
			if strings.Contains(h.transport.text(), key) {
				t.Errorf("%s: the key left the process", mode)
			}
		}
		span := spans["invoke_agent support-bot"]
		if got := span["status"].(map[string]any)["message"]; got != failure {
			t.Errorf("%s: the status message sent %q, want %q", mode, got, failure)
		}
		for _, raw := range span["events"].([]any) {
			event := raw.(map[string]any)
			if event["name"] != "exception" {
				continue
			}
			for _, a := range event["attributes"].([]any) {
				attribute := a.(map[string]any)
				if got := attribute["value"].(map[string]any)["stringValue"]; attribute["key"] == "exception.message" && got != failure {
					t.Errorf("%s: exception.message sent %q, want %q", mode, got, failure)
				}
			}
		}
		expectScrubbed(t, mode+": the run", span, counted)
	}
}

// The scrubbers see what the mask answered, and nothing it dropped.
func TestTheScrubbersRunOnWhatTheMaskKept(t *testing.T) {
	unsetScrubbing(t)
	mask := func(key, value string) (string, bool) {
		if key == "gen_ai.tool.call.arguments" {
			return "", false
		}
		return value + " AKIAIOSFODNN7EXAMPLE", true
	}
	h := newHarness(t, telmoni.Config{APIKey: telmoni.NewSecret("k"), CaptureContent: true, Mask: mask}, newFakeTransport(t, contentOn(), contentOn()))
	aRun(t, h)
	ctx, run := h.client.Run(context.Background(), "support-bot", telmoni.RunOptions{})
	_, tool := run.ToolCall(ctx, "read_file", telmoni.ToolCallOptions{Arguments: "key ASIAIOSFODNN7EXAMPLE"})
	tool.Returned("done")
	tool.End()
	run.End()
	h.flush()
	span := tools(t, h)[0]
	attributes := attributesOf(t, span)
	if got := attributes["gen_ai.tool.call.result"]; got != "done [secret: aws_key]" {
		t.Errorf("gen_ai.tool.call.result sent %q, want the mask's answer scrubbed", got)
	}
	if _, sent := attributes["gen_ai.tool.call.arguments"]; sent {
		t.Error("a field the mask dropped was sent")
	}
	expectScrubbed(t, "the call", span, map[string]int{"secret:aws_key": 1})
}

// An image is lifted out of its message before the scrubbers run, so its
// base64 is put whole and never scanned; the text beside it is scrubbed.
func TestAnImagesBase64IsLiftedAndNeverScrubbed(t *testing.T) {
	unsetScrubbing(t)
	// Valid base64, and a key's shape the AWS rule would replace.
	const key = "AKIAIOSFODNN7EXAMPLE"
	h := newHarness(t, telmoni.Config{APIKey: telmoni.NewSecret("k"), CaptureContent: true}, newFakeTransport(t, contentOn(), contentOn()))
	aRun(t, h)
	ctx, run := h.client.Run(context.Background(), "support-bot", telmoni.RunOptions{})
	_, call := run.ModelCall(ctx, "gpt-4o", telmoni.ModelCallOptions{InputMessages: []map[string]any{{
		"role": "user",
		"parts": []map[string]any{
			{"type": "blob", "modality": "image", "mime_type": "image/png", "content": key},
			{"type": "text", "content": "key " + key},
		},
	}}})
	call.End()
	run.End()
	h.flush()
	image, err := base64.StdEncoding.DecodeString(key)
	if err != nil {
		t.Fatalf("the image's base64: %v", err)
	}
	if uploads := h.transport.allUploads(); len(uploads) != 1 || !bytes.Equal(uploads[0].body, image) {
		t.Fatalf("put %d files, want the image's bytes once", len(uploads))
	}
	sum := sha256.Sum256(image)
	span := withPrefix(h.transport.spans(t), "chat ")[0]
	var sent any
	if err := json.Unmarshal([]byte(attributesOf(t, span)["gen_ai.input.messages"].(string)), &sent); err != nil {
		t.Fatalf("gen_ai.input.messages is no JSON: %v", err)
	}
	want := []any{map[string]any{"role": "user", "parts": []any{
		map[string]any{"type": "file", "file_id": "telmoni:" + hex.EncodeToString(sum[:]), "modality": "image", "mime_type": "image/png"},
		map[string]any{"type": "text", "content": "key [secret: aws_key]"},
	}}}
	if !reflect.DeepEqual(sent, want) {
		t.Errorf("sent\n%s\nwant\n%s", pretty(t, sent), pretty(t, want))
	}
	expectScrubbed(t, "the call", span, map[string]int{"secret:aws_key": 1})
}

// A recorded run's content is scrubbed as a live run's is.
func TestARecordedRunsContentLeavesScrubbed(t *testing.T) {
	unsetScrubbing(t)
	prose := loadCanary(t).named(t, "every planted value in one prose text")
	h := newHarness(t, telmoni.Config{APIKey: telmoni.NewSecret("k"), CaptureContent: true}, newFakeTransport(t, contentOn(), contentOn()))
	aRun(t, h)
	at := time.Date(2026, 11, 5, 9, 0, 0, 0, time.UTC)
	recordRun(t, h, telmoni.RunRecord{
		RunID: "2f0b7c4e-9d1a-4e6b-8c3f-5a7d9e1b2c40", Agent: "claude-code", Start: at, End: at,
		Inside: []telmoni.RecordItem{telmoni.ToolCallRecord{Name: "read_file", Start: at, End: at, Result: prose.Text}},
	})
	span := tools(t, h)[0]
	want := prose.Expect["sdk_default"]
	if got := attributesOf(t, span)["gen_ai.tool.call.result"]; got != want.Text {
		t.Errorf("gen_ai.tool.call.result sent %q, want %q", got, want.Text)
	}
	expectScrubbed(t, "the recorded call", span, want.Scrubbed)
}

// An unset switch is read from its variable, White_Space trimmed: only 0 turns
// the secrets' off and only 1 the personal data's on. One set in code stands.
func TestTheScrubbersSwitchesAreReadFromTheEnvironmentUnlessSetInCode(t *testing.T) {
	t.Setenv("TELMONI_ENDPOINT", "")
	const unset = "<unset>"
	for _, c := range []struct {
		secrets, pii         string
		code                 telmoni.Config
		wantSecrets, wantPII bool
	}{
		{unset, unset, telmoni.Config{}, true, false},
		{"0", "1", telmoni.Config{}, false, true},
		{" 0\t", " 1\n", telmoni.Config{}, false, true},
		{"", "", telmoni.Config{}, true, false},
		{"false", "true", telmoni.Config{}, true, false},
		{"off", "on", telmoni.Config{}, true, false},
		{"00", "01", telmoni.Config{}, true, false},
		{"1", "0", telmoni.Config{}, true, false},
		{"0", "1", telmoni.Config{ScrubSecrets: new(true), ScrubPII: new(false)}, true, false},
		{unset, unset, telmoni.Config{ScrubSecrets: new(false), ScrubPII: new(true)}, false, true},
	} {
		for name, value := range map[string]string{"TELMONI_SCRUB_SECRETS": c.secrets, "TELMONI_SCRUB_PII": c.pii} {
			t.Setenv(name, value)
			if value == unset {
				os.Unsetenv(name)
			}
		}
		fromEnv := telmoni.ConfigFromEnv()
		if c.code.ScrubSecrets != nil || c.code.ScrubPII != nil {
			fromEnv.ScrubSecrets, fromEnv.ScrubPII = c.code.ScrubSecrets, c.code.ScrubPII
		}
		for how, cfg := range map[string]telmoni.Config{"New": mustNew(t, c.code).Config(), "NewFromEnv": mustNew(t, fromEnv).Config()} {
			if *cfg.ScrubSecrets != c.wantSecrets || *cfg.ScrubPII != c.wantPII {
				t.Errorf("%s with %q, %q and %+v in code: secrets %t, PII %t; want %t, %t",
					how, c.secrets, c.pii, c.code, *cfg.ScrubSecrets, *cfg.ScrubPII, c.wantSecrets, c.wantPII)
			}
		}
	}

	t.Setenv("TELMONI_SCRUB_SECRETS", "0")
	t.Setenv("TELMONI_SCRUB_PII", "1")
	h := newHarness(t, telmoni.Config{APIKey: telmoni.NewSecret("k"), CaptureContent: true}, newFakeTransport(t, contentOn(), contentOn()))
	aRun(t, h)
	ctx, run := h.client.Run(context.Background(), "support-bot", telmoni.RunOptions{})
	_, tool := run.ToolCall(ctx, "read_file", telmoni.ToolCallOptions{})
	tool.Returned("mail dana.whitfield@example.com, key AKIAIOSFODNN7EXAMPLE")
	tool.End()
	run.End()
	h.flush()
	span := tools(t, h)[0]
	if got := attributesOf(t, span)["gen_ai.tool.call.result"]; got != "mail [pii: email], key AKIAIOSFODNN7EXAMPLE" {
		t.Errorf("with the secrets' off and the personal data's on, sent %q", got)
	}
	expectScrubbed(t, "the call", span, map[string]int{"pii:email": 1})
}

// A field the scrubbers fail on goes, named, rather than leave unscrubbed.
func TestAFieldTheScrubbersFailOnIsDroppedAndNamed(t *testing.T) {
	telmoni.FailScrubbing(t)
	unsetScrubbing(t)
	h := newHarness(t, telmoni.Config{APIKey: telmoni.NewSecret("k"), CaptureContent: true}, newFakeTransport(t, contentOn(), contentOn()))
	aRun(t, h)
	ctx, run := h.client.Run(context.Background(), "support-bot", telmoni.RunOptions{})
	_, tool := run.ToolCall(ctx, "read_file", telmoni.ToolCallOptions{})
	tool.Returned("key AKIAIOSFODNN7EXAMPLE")
	tool.End()
	run.End()
	h.flush()
	span := tools(t, h)[0]
	if got, sent := attributesOf(t, span)["gen_ai.tool.call.result"]; sent {
		t.Errorf("the field the scrubbers failed on was sent: %v", got)
	}
	expectScrubbed(t, "the call", span, map[string]int{"dropped:gen_ai.tool.call.result": 1})
	if strings.Contains(h.transport.text(), "AKIAIOSFODNN7EXAMPLE") || strings.Contains(strings.Join(h.logs.warnings(), "\n"), "AKIAIOSFODNN7EXAMPLE") {
		t.Error("what the scrubbers failed on left the process")
	}
}
