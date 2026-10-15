# Controller configuration

`server.env.example` is a portable template for system deployment. Set either `DATABASE_URL`
or the standard `PG*` settings; URL takes priority. Local/remote PostgreSQL is supported;
the platform does not require a specific machine, port or OS user.

Copy outside the repository and chmod 600. Actual secrets, production inventory and
database files must never be committed. See `deploy/database/README.md`.

