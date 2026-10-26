use dzban_core::adapter::{AdapterFactory, DatabaseSession};
use dzban_core::result_store::ResultLimits;
use dzban_core::result_stream::{ResultPhase, ResultTermination, start_results};
use dzban_core::{DatabaseConfig, ResultCell};
use dzban_db::adapters::PostgresqlAdapter;

async fn rows(session: &dyn DatabaseSession, sql: &str) -> dzban_core::ResultSet {
    session
        .execute_detailed(sql)
        .await
        .unwrap_or_else(|error| panic!("{sql}: {error}"))
        .into_parts()
        .0
}

#[tokio::test]
#[ignore = "requires PostgreSQL 15, 16 and 17 from docker-compose.test.yml"]
async fn advanced_values_round_trip_through_the_shared_adapter() {
    for connection in [
        "host=localhost port=5415 user=testuser password=testpass dbname=testdb",
        "host=localhost port=5416 user=user16 password=pass16 dbname=proddb",
        "host=localhost port=5417 user=admin password=secret dbname=devdb",
    ] {
        let session = PostgresqlAdapter
            .create(DatabaseConfig::from_connection_string(connection))
            .unwrap();
        session.connect_detailed().await.unwrap();
        session
            .execute_detailed("CREATE EXTENSION IF NOT EXISTS hstore")
            .await
            .unwrap();
        session.execute_detailed("CREATE TEMP TABLE value_record (id integer, note text, attributes hstore, amount money)").await.unwrap();
        session
            .execute_detailed("CREATE DOMAIN pg_temp.attributes AS hstore")
            .await
            .unwrap();
        session
            .execute_detailed("CREATE TYPE pg_temp.rating AS ENUM ('good', 'bad')")
            .await
            .unwrap();
        session.execute_detailed("CREATE TEMP TABLE nested_values (item pg_temp.value_record, attributes pg_temp.attributes, ratings pg_temp.rating[])").await.unwrap();
        for (expression, type_) in [
            (
                "hstore(ARRAY['b','a','null'], ARRAY['žluť',NULL,'NULL'])",
                "hstore",
            ),
            ("''::hstore", "hstore"),
            ("hstore('key', 'quote\" and slash\\')", "hstore"),
            (
                "hstore('key','value')::pg_temp.attributes",
                "pg_temp.attributes",
            ),
            ("ARRAY[hstore('a','b'), NULL, ''::hstore]", "hstore[]"),
            ("ARRAY[['', NULL], ['NULL', 'a,b']]", "text[]"),
            ("'[0:1][-2:-1]={{1,2},{3,4}}'::int[]", "int[]"),
            ("ARRAY[]::text[]", "text[]"),
            ("ARRAY['good'::pg_temp.rating, 'bad']", "pg_temp.rating[]"),
            (
                "ROW(1, E'quote\" slash\\\\ newline\\n', hstore('k',NULL), 123.45::money)::pg_temp.value_record",
                "pg_temp.value_record",
            ),
            (
                "ROW(NULL, '', ''::hstore, NULL)::pg_temp.value_record",
                "pg_temp.value_record",
            ),
            (
                "ARRAY[ROW(1,'abc',hstore('a','b'),1::money)::pg_temp.value_record]",
                "pg_temp.value_record[]",
            ),
            (
                "ROW(ROW(2,'nested',hstore('a','b'),(-12.34)::money)::pg_temp.value_record, hstore('x',NULL)::pg_temp.attributes, ARRAY['good'::pg_temp.rating])::pg_temp.nested_values",
                "pg_temp.nested_values",
            ),
            ("'[1,10)'::int4range", "int4range"),
            ("'(,)'::int8range", "int8range"),
            ("'empty'::numrange", "numrange"),
            ("'[1.25,9.75]'::numrange", "numrange"),
            ("'[2026-01-01,2026-02-01)'::daterange", "daterange"),
            (
                "'[2026-01-01 12:34:56,2026-02-01 12:34:56)'::tsrange",
                "tsrange",
            ),
            (
                "'[2026-01-01 12:34:56+00,infinity)'::tstzrange",
                "tstzrange",
            ),
            ("'{[1,3),[7,9)}'::int4multirange", "int4multirange"),
            ("'{}'::datemultirange", "datemultirange"),
            ("'{(,10)}'::int8multirange", "int8multirange"),
            ("'{[1.25,9.75]}'::nummultirange", "nummultirange"),
            ("'{[2026-01-01,2026-02-01)}'::tsmultirange", "tsmultirange"),
            (
                "'{[2026-01-01,2026-02-01)}'::tstzmultirange",
                "tstzmultirange",
            ),
            ("ARRAY['[1,5)'::int4range,NULL]", "int4range[]"),
            ("'(1.5,-2)'::point", "point"),
            ("'(5,6),(1,2)'::box", "box"),
            ("ARRAY['(5,6),(1,2)'::box,'(9,9),(7,7)'::box]", "box[]"),
            ("'[(1,2),(3,4)]'::path", "path"),
            ("'((1,2),(3,4))'::path", "path"),
            ("'{1,2,3}'::line", "line"),
            ("'[(1,2),(3,4)]'::lseg", "lseg"),
            ("'((1,2),(3,4),(5,6))'::polygon", "polygon"),
            ("'<(1,2),3>'::circle", "circle"),
            ("'''cat'':1A,2B,3C,4 ''dog'''::tsvector", "tsvector"),
            ("''::tsvector", "tsvector"),
            ("''::tsquery", "tsquery"),
            ("'cat:A* & !dog | (rat <2> mouse)'::tsquery", "tsquery"),
            ("'one <-> (two <-> three)'::tsquery", "tsquery"),
            ("'$.items[*] ? (@.price > 10)'::jsonpath", "jsonpath"),
            ("123.45::money", "money"),
            ("(-0.01)::money", "money"),
            ("'-92233720368547758.08'::money", "money"),
            ("ARRAY[1::money,2::money,NULL]", "money[]"),
            ("'10:20:11,18'::pg_snapshot", "pg_snapshot"),
            ("'10:20:'::txid_snapshot", "txid_snapshot"),
            ("'1 2 -3'::int2vector", "int2vector"),
            ("'0 42 4294967295'::oidvector", "oidvector"),
            ("ARRAY['1 2'::int2vector,'3 4'::int2vector]", "int2vector[]"),
        ] {
            let result = rows(
                session.as_ref(),
                &format!("SELECT {expression} AS value, ({expression})::text AS oracle"),
            )
            .await;
            let ResultCell::Text(value) = &result.rows[0][0] else {
                panic!("non-null {type_}");
            };
            let literal = value.replace('\'', "''");
            let normalized = rows(
                session.as_ref(),
                &format!("SELECT ('{literal}'::{type_})::text"),
            )
            .await;
            assert_eq!(
                normalized.rows[0][0], result.rows[0][1],
                "{connection}: {expression} decoded as {value}"
            );
            let (mut sink, reader) = start_results(ResultLimits {
                memory_rows: 1,
                ..Default::default()
            });
            let outcome = session
                .execute_streamed(
                    &format!("SELECT {expression} AS value FROM generate_series(1,3)"),
                    &mut sink,
                )
                .await;
            sink.finish(outcome.map(|_| ())).await.unwrap();
            let snapshot = reader.borrow().clone();
            assert_eq!(
                snapshot.phase,
                ResultPhase::Finished(ResultTermination::Complete),
                "{expression}"
            );
            let page = snapshot.result.unwrap().read_page(2, 1).await.unwrap();
            assert_eq!(page.columns[0], result.columns[0]);
            assert_eq!(page.rows[0][0], result.rows[0][0], "streamed {expression}");
        }
        let record = rows(
            session.as_ref(),
            "SELECT ROW(1, 'abc'::text, NULL::int, 12.34::money)",
        )
        .await;
        assert_eq!(record.rows[0][0], ResultCell::Text("(1,abc,,12.34)".into()));
        let nulls = rows(
            session.as_ref(),
            "SELECT NULL::hstore, NULL::int4range, NULL::pg_temp.value_record",
        )
        .await;
        assert_eq!(nulls.rows[0], vec![ResultCell::Null; 3]);
        let sql = "SELECT hstore('key', i::text) AS attributes FROM generate_series(1,250) i";
        let (mut sink, reader) = start_results(ResultLimits {
            memory_rows: 1,
            ..Default::default()
        });
        let outcome = session.execute_streamed(sql, &mut sink).await;
        sink.finish(outcome.map(|_| ())).await.unwrap();
        let snapshot = reader.borrow().clone();
        assert_eq!(
            snapshot.phase,
            ResultPhase::Finished(ResultTermination::Complete)
        );
        let page = snapshot.result.unwrap().read_page(249, 1).await.unwrap();
        assert_eq!(page.columns[0].type_name, "hstore");
        assert_eq!(page.rows[0][0], ResultCell::Text("\"key\"=>\"250\"".into()));
        session.disconnect().await.unwrap();
    }
}
