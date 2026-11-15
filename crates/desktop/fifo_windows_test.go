package main

import "fmt"

func credentialFIFO(string) error { return fmt.Errorf("Unix FIFO is unavailable on Windows") }
