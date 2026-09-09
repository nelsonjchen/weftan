// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

// Command oss-tala-oracle exposes the pinned OSS TALA layout through the
// serialized graph interface used by Weftan's parity comparators.
package main

import (
	"context"
	"flag"
	"fmt"
	"io"
	"os"
	"strconv"
	"strings"

	"github.com/d2lang/d2/d2graph"
	"github.com/d2lang/d2/d2layouts/d2talalayout"
)

func run() error {
	if len(os.Args) < 2 || os.Args[1] != "layout" {
		return fmt.Errorf("usage: oss-tala-oracle layout --tala-seeds 1[,2,...] < graph.json")
	}
	flags := flag.NewFlagSet("layout", flag.ContinueOnError)
	rawSeeds := flags.String("tala-seeds", "1", "comma-separated signed 64-bit seeds")
	if err := flags.Parse(os.Args[2:]); err != nil {
		return err
	}
	var seeds []int64
	for _, raw := range strings.Split(*rawSeeds, ",") {
		seed, err := strconv.ParseInt(strings.TrimSpace(raw), 10, 64)
		if err != nil {
			return err
		}
		seeds = append(seeds, seed)
	}
	input, err := io.ReadAll(os.Stdin)
	if err != nil {
		return err
	}
	var graph d2graph.Graph
	if err := d2graph.DeserializeGraph(input, &graph); err != nil {
		return err
	}
	if err := d2talalayout.Layout(context.Background(), &graph, &d2talalayout.Options{Seeds: seeds, MaxConcurrency: 1}); err != nil {
		return err
	}
	output, err := d2graph.SerializeGraph(&graph)
	if err != nil {
		return err
	}
	_, err = os.Stdout.Write(output)
	return err
}

func main() {
	if err := run(); err != nil {
		fmt.Fprintln(os.Stderr, err)
		os.Exit(1)
	}
}
