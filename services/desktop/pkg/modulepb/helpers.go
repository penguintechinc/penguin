package modulepb

import "fmt"

// UnknownCommandResponse returns a CLICommandResponse for an unrecognized command path.
func UnknownCommandResponse(path string) *CLICommandResponse {
	return &CLICommandResponse{
		Stderr:   fmt.Sprintf("unknown command: %s\n", path),
		ExitCode: 1,
	}
}

// ErrorResponse returns a CLICommandResponse wrapping an error.
func ErrorResponse(err error) *CLICommandResponse {
	return &CLICommandResponse{
		Stderr:   fmt.Sprintf("error: %v\n", err),
		ExitCode: 1,
	}
}

// OKResponse returns a successful CLICommandResponse with the given output.
func OKResponse(stdout string) *CLICommandResponse {
	return &CLICommandResponse{
		Stdout: stdout,
	}
}
