package sdk

// CommandSpec declares one CLI command in a module's command tree. It is pure
// data: the penguin CLI renders it with cobra and routes execution back to
// Module.Dispatch with the command's path.
type CommandSpec struct {
	// Name is the command token as typed ("connect", "query").
	Name string
	// Use is the one-line usage string ("query <domain>").
	Use string
	// Short is the help summary.
	Short string
	// Flags declares the command's flags.
	Flags []FlagSpec
	// Subcommands nest further commands; leaves are dispatchable.
	Subcommands []CommandSpec
	// Tray marks commands surfaced as tray menu actions.
	Tray bool
	// MinArgs/MaxArgs bound positional args (MaxArgs -1 = unlimited).
	MinArgs int
	MaxArgs int
}

// FlagSpec declares a single command flag.
type FlagSpec struct {
	Name      string
	Shorthand string
	Usage     string
	Default   string
	Type      FlagType
}

// FlagType is the flag's value type for CLI parsing.
type FlagType string

const (
	FlagString FlagType = "string"
	FlagBool   FlagType = "bool"
	FlagInt    FlagType = "int"
)

// Result is the outcome of a Dispatch invocation.
type Result struct {
	// Output is human-readable text for the terminal.
	Output string
	// JSON optionally carries machine-readable output (used by --json).
	JSON []byte
	// ExitCode is the process exit code the CLI should use (0 = success).
	ExitCode int
}
