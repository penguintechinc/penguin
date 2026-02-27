package pluginhost

import (
	"io"
	"log"

	"github.com/hashicorp/go-hclog"
	"github.com/sirupsen/logrus"
)

// hclogAdapter wraps a logrus.Logger to satisfy the hclog.Logger interface
// required by hashicorp/go-plugin.
type hclogAdapter struct {
	logger *logrus.Logger
	name   string
	args   []interface{}
}

func newHCLogAdapter(logger *logrus.Logger, name string) hclog.Logger {
	return &hclogAdapter{logger: logger, name: name}
}

func (h *hclogAdapter) Log(level hclog.Level, msg string, args ...interface{}) {
	entry := h.logger.WithField("plugin", h.name)
	switch level {
	case hclog.Trace, hclog.Debug:
		entry.Debug(msg)
	case hclog.Info:
		entry.Info(msg)
	case hclog.Warn:
		entry.Warn(msg)
	case hclog.Error:
		entry.Error(msg)
	}
}

func (h *hclogAdapter) Trace(msg string, args ...interface{}) { h.Log(hclog.Trace, msg, args...) }
func (h *hclogAdapter) Debug(msg string, args ...interface{}) { h.Log(hclog.Debug, msg, args...) }
func (h *hclogAdapter) Info(msg string, args ...interface{})  { h.Log(hclog.Info, msg, args...) }
func (h *hclogAdapter) Warn(msg string, args ...interface{})  { h.Log(hclog.Warn, msg, args...) }
func (h *hclogAdapter) Error(msg string, args ...interface{}) { h.Log(hclog.Error, msg, args...) }

func (h *hclogAdapter) IsTrace() bool { return h.logger.IsLevelEnabled(logrus.TraceLevel) }
func (h *hclogAdapter) IsDebug() bool { return h.logger.IsLevelEnabled(logrus.DebugLevel) }
func (h *hclogAdapter) IsInfo() bool  { return h.logger.IsLevelEnabled(logrus.InfoLevel) }
func (h *hclogAdapter) IsWarn() bool  { return h.logger.IsLevelEnabled(logrus.WarnLevel) }
func (h *hclogAdapter) IsError() bool { return h.logger.IsLevelEnabled(logrus.ErrorLevel) }

func (h *hclogAdapter) ImpliedArgs() []interface{} { return h.args }

func (h *hclogAdapter) With(args ...interface{}) hclog.Logger {
	return &hclogAdapter{logger: h.logger, name: h.name, args: append(h.args, args...)}
}

func (h *hclogAdapter) Name() string { return h.name }

func (h *hclogAdapter) Named(name string) hclog.Logger {
	return &hclogAdapter{logger: h.logger, name: h.name + "." + name, args: h.args}
}

func (h *hclogAdapter) ResetNamed(name string) hclog.Logger {
	return &hclogAdapter{logger: h.logger, name: name, args: h.args}
}

func (h *hclogAdapter) SetLevel(level hclog.Level) {}

func (h *hclogAdapter) GetLevel() hclog.Level {
	switch h.logger.GetLevel() {
	case logrus.TraceLevel:
		return hclog.Trace
	case logrus.DebugLevel:
		return hclog.Debug
	case logrus.InfoLevel:
		return hclog.Info
	case logrus.WarnLevel:
		return hclog.Warn
	case logrus.ErrorLevel, logrus.FatalLevel, logrus.PanicLevel:
		return hclog.Error
	default:
		return hclog.Info
	}
}

func (h *hclogAdapter) StandardLogger(opts *hclog.StandardLoggerOptions) *log.Logger {
	return log.New(h.StandardWriter(opts), "", 0)
}

func (h *hclogAdapter) StandardWriter(opts *hclog.StandardLoggerOptions) io.Writer {
	return h.logger.Writer()
}
