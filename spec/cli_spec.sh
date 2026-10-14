Describe 'dzb process contracts'
  It 'shows help without reading configuration'
    When run env DZBAN_CONFIG=/does-not-exist target/debug/dzb
    The status should equal 0
    The output should include 'Workflow:'
    The output should include '[aliases: c]'
    The stderr should equal ''
  End

  It 'prints version without connecting'
    When run env DZBAN_CONFIG=/does-not-exist target/debug/dzb v
    The status should equal 0
    The output should include 'dzb'
    The stderr should equal ''
  End

  It 'rejects a console without a terminal'
    When run target/debug/dzb c
    The status should equal 2
    The stderr should include 'requires a terminal'
    The stdout should equal ''
  End

  It 'submits, lists tables, quits and restores the terminal on both adapters'
    Skip if 'disposable databases are not enabled' test "${DZBAN_TEST_DATABASES:-}" != 1
    When run python3 scripts/test-cli-pty.py
    The status should equal 0
    The output should include 'terminal cleanup passed'
    The stderr should equal ''
  End
End
