def foo():
    # NOTE: A note that applies to all of them.
    # The note can be multi-line as well.
    import thing
    import blah

    import another

    ... notice that there's no comment over `import another`. That's because the whitespace is ignored and it's treated as all "one group of noted imports"
