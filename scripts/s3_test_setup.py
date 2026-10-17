#!/usr/bin/env python3
"""Prepare a freshly started moto server for the S3 tests (used by s3-test.sh).

    s3_test_setup.py ENDPOINT BUCKET

Creates an IAM user with full S3 access and an access key, creates the bucket,
then turns signature checking ON for every later request (moto's IAM
enforcement: it recomputes the SigV4 signature of each request with botocore
and rejects a mismatch). Prints the shell assignments for the credentials.
"""
import json
import sys
import urllib.request

import boto3
from botocore.config import Config

endpoint, bucket = sys.argv[1], sys.argv[2]
region = "us-east-1"

common = dict(endpoint_url=endpoint, region_name=region, aws_access_key_id="bootstrap", aws_secret_access_key="bootstrap")
iam = boto3.client("iam", **common)
iam.create_user(UserName="ag-test")
iam.put_user_policy(
    UserName="ag-test",
    PolicyName="s3-all",
    PolicyDocument=json.dumps({"Version": "2012-10-17", "Statement": [{"Effect": "Allow", "Action": "s3:*", "Resource": "*"}]}),
)
key = iam.create_access_key(UserName="ag-test")["AccessKey"]

s3 = boto3.client("s3", config=Config(s3={"addressing_style": "path"}), **common)
s3.create_bucket(Bucket=bucket)

# From now on every request must carry a valid signature of a known access key.
req = urllib.request.Request(endpoint + "/moto-api/reset-auth", data=b"0", method="POST", headers={"Content-Type": "text/plain"})
urllib.request.urlopen(req).read()

print(f"AG_TEST_S3_ACCESS_KEY_ID={key['AccessKeyId']}")
print(f"AG_TEST_S3_SECRET_ACCESS_KEY={key['SecretAccessKey']}")
