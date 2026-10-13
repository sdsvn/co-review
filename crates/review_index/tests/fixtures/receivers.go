package main

type Ünit struct{}

type Ü struct{}

// The TypeScript qualifies with the ASCII tail of the receiver type only.
func (u *Ünit) Run() {}

// No ASCII identifier before `)`: the method is not qualified.
func (u Ü) Stop() {}
