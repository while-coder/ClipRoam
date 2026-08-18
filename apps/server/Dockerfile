FROM node:24-alpine

ENV NODE_ENV=production
ENV CLIPROAM_PORT=4810

WORKDIR /app

COPY . .

VOLUME ["/root/.cliproam"]

EXPOSE 4810

CMD ["node", "build/index.js"]
