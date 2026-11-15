package main

import (
	"context"
	"flag"
	"fmt"
	"os"
	"os/signal"

	"github.com/listenbox/client/internal/nativebuild"
)

func main() {
	if err := run(); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}

func run() error {
	check := flag.Bool("check", false, "Verify native compiler and SDK tools")
	flag.Parse()
	ctx, cancel := signal.NotifyContext(context.Background(), os.Interrupt)
	defer cancel()
	var err error
	if *check {
		var toolchain nativebuild.Toolchain
		toolchain, err = nativebuild.HostToolchain()
		if err == nil {
			err = toolchain.Check(ctx)
		}
	} else {
		var root string
		root, err = os.Getwd()
		if err == nil {
			err = nativebuild.Build(ctx, root)
		}
	}
	return err
}
