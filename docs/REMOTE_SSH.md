# Remote SSH Model

Ashide treats SSH hosts as persistent environments, not just one-off terminal commands.

## Goals

- Use existing OpenSSH configuration as the primary source of hosts.
- Connect first, then read the remote environment.
- Keep local and remote state separate.
- Make the remote environment feel local after connection.
- Support terminal, project navigation, file browsing, and agent session discovery from the remote machine.

## Mental model

A connected SSH environment should behave like a workspace context:

1. The user selects an SSH host.
2. Ashide establishes the remote runtime.
3. Ashide scans the remote machine for supported agent sessions.
4. New terminals open inside that remote environment.
5. Project/file views read remote files, not local files.

This is inspired by VS Code Remote SSH, but Ashide keeps the terminal and CLI agents at the center.

## Project explorer vs file browser

Ashide keeps two separate concepts:

- **Project explorer**: current workspace/project root. It follows the active project/cwd.
- **File browser**: broader machine-level navigation, usually the user's home directory.

For remote environments, the project explorer may be backed by a remote file-browser implementation internally, but the user-facing semantics remain different.

## Current status

Remote SSH support is experimental and under active iteration.

Known active areas:

- environment lifecycle and disconnect behavior;
- cwd-following for remote project roots;
- remote file/project UI polish;
- robust remote agent session discovery;
- clearer connection state feedback.
