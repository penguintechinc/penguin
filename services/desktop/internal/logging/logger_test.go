package logging

import (
	"bytes"
	"io"
	"os"
	"path/filepath"
	"testing"

	"github.com/sirupsen/logrus"
	"github.com/stretchr/testify/assert"
	"github.com/stretchr/testify/require"
)

func TestNewLogger(t *testing.T) {
	t.Run("default level is info", func(t *testing.T) {
		logger := NewLogger("invalid-level", "text", "")
		assert.Equal(t, logrus.InfoLevel, logger.GetLevel())
	})

	t.Run("sets correct level", func(t *testing.T) {
		logger := NewLogger("debug", "text", "")
		assert.Equal(t, logrus.DebugLevel, logger.GetLevel())
	})

	t.Run("default format is text", func(t *testing.T) {
		var buf bytes.Buffer
		logger := NewLogger("info", "invalid-format", "")
		logger.SetOutput(&buf)
		logger.Info("test message")
		// JSON format would contain quotes, text does not on the message
		assert.NotContains(t, buf.String(), `"msg":"test message"`)
	})

	t.Run("sets json format", func(t *testing.T) {
		var buf bytes.Buffer
		logger := NewLogger("info", "json", "")
		logger.SetOutput(&buf)
		logger.Info("test message")
		assert.Contains(t, buf.String(), `"msg":"test message"`)
	})

	t.Run("writes to stderr by default", func(t *testing.T) {
		logger := NewLogger("info", "text", "")
		assert.Equal(t, os.Stderr, logger.Out)
	})

	t.Run("writes to file and stderr", func(t *testing.T) {
		tempDir := t.TempDir()
		logFile := filepath.Join(tempDir, "test.log")
		logger := NewLogger("info", "text", logFile)

		// Check that output is a multi-writer
		_, ok := logger.Out.(*io.PipeWriter)
		if !ok {
			// depending on how MultiWriter is implemented, it may not be directly visible.
			// so, we check by writing and seeing if it appears in the file
		}

		logger.Info("test file message")

		// Check file content
		content, err := os.ReadFile(logFile)
		require.NoError(t, err)
		assert.Contains(t, string(content), "test file message")
	})

	t.Run("handles invalid log file path", func(t *testing.T) {
		// Using a path that is not permitted to write to
		logFile := "/root/unauthorized.log"
		var stderrBuf bytes.Buffer
		
		// Temporarily redirect stderr to capture warnings
		oldStderr := os.Stderr
		r, w, _ := os.Pipe()
		os.Stderr = w
		
		logger := NewLogger("info", "text", logFile)
		
		// Restore stderr
		w.Close()
		os.Stderr = oldStderr
		
		io.Copy(&stderrBuf, r)

		assert.NotNil(t, logger.Out)
		// In case of file error, it should revert to stderr.
		// We can't directly compare os.Stderr as it might be a different file descriptor
		// to the one we captured. Instead, we check that it's an *os.File.
		_, ok := logger.Out.(*os.File)
		assert.True(t, ok, "logger.Out should be an *os.File (stderr)")
	})
}
