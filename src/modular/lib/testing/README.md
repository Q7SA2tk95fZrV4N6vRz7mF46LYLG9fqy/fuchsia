Here is code that helps clients of modular with testing.

When linked against the `//src/modular/lib/testing` library target, a test
application can use the functions in `lib/testing/testing.h` to interact with
the `TestRunner` service in its environment. This is the standard way to test
multi-process modular applications; these functions allow a test to declare
failure, and signal tear down of tests to the TestRunner service.

See [//src/modular/tests](/src/modular/tests) for more details on running
tests.
