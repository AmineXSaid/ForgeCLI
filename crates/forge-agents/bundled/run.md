---
name: run
description: Start the project and show it working
disable-model-invocation: true
---
Start this project and show that it works.

1. Find out how it runs. Look at a project skill or FORGE.md notes first, then the README, package scripts, Makefile, Procfile, docker-compose file or the main entry point. Install dependencies only if the project's own instructions say how.
2. Start it. Run a server or other long-running process in the background, and wait until it is ready: a log line, or a port that answers.
3. Exercise it: request the main endpoint, run the CLI's main command, or open the main page's HTML. Show the actual output.
4. Stop anything you started in the background, and report the commands that worked.

If it can't start (missing services, secrets or setup), say exactly what is missing and how to provide it.
